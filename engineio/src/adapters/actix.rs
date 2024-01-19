use std::sync::Arc;
use tokio_stream::StreamExt;
use actix_web::{guard, web, HttpResponse, Resource};
use actix_ws::{Message};

use crate::{async_session_io_create, EngineInput, session_collect, EngineInputError};
use crate::engine::{Sid, WebsocketEvent, TransportConfig, Payload, Participant };
use crate::proto::EngineError;
pub use super::common::{ NewConnectionService, Emitter };

impl From<actix_ws::Message> for WebsocketEvent {
    fn from(value: actix_ws::Message) -> Self {
        Self::Ping
    }
}

//#[cfg(feature = "actix")]
//impl From<Result<actix_ws::Message, actix_ws::ProtocolError>> for WebsocketEvent {
//    fn from(value: Result<actix_ws::Message, actix_ws::ProtocolError>) -> Self {
//        Self::Ping
//    }
//}

#[derive(serde::Deserialize)]
struct SessionInfo {
    #[serde(alias = "EIO")]
    eio: u8,
    sid: Option<Sid> 
}

impl actix_web::ResponseError for EngineError{
    fn status_code(&self) -> actix_web::http::StatusCode {
        match self {
            EngineError::InvalidPollRequest => actix_web::http::StatusCode::BAD_REQUEST,
            EngineError::Generic => actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
            EngineError::MissingSession => actix_web::http::StatusCode::BAD_REQUEST,
            _ => actix_web::http::StatusCode::NOT_FOUND
        }
    }

    fn error_response(&self) -> HttpResponse<actix_web::body::BoxBody> {
        return HttpResponse::new(self.status_code())
    }

}

//async fn create_sio_ws<F>(req: HttpRequest, body: web::Payload, handler:F)-> impl Responder 
//where F:  Fn(AsyncEngine, Emitter) -> (),
//{
//    let engine = Engine::new_ws();
//    let (response, session, msg_stream) = actix_ws::handle(&req, body).unwrap();
//
//    let config = SessionConfig::default();
//    let res = json!({
//      "sid": engine.session,
//      "upgrades": [],
//      "pingInterval": config.ping_interval,
//      "pingTimeout": config.ping_timeout,
//      "maxPayload": config.max_payload
//    });
//
//    let emitter = Emitter::WS(session);
//    let sio = AsyncEngine::WS(AsyncEngineInner::new(engine, msg_stream));
//    handler(sio,emitter);
//
//    return web::Bytes::from(res.to_string());
//}

// Actix Service for accepting LONG POLL reqs and WS ? 
pub fn socket_io<F>(path:actix_web::Resource, config:TransportConfig, callback: F) -> Resource
where F: NewConnectionService + 'static
{
    let client = Arc::new(callback);
    let io = async_session_io_create();

    // WS 
    let path = {
        let client = client.clone();
        let io = io.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .guard(guard::fn_guard(|ctx| {
                ctx.head().headers().get("Upgrade").is_some_and(|v| v == "websocket")
            }))
            .to(move |req: actix_web::HttpRequest, body: web::Payload| { 
                let client = client.clone();
                let io = io.clone();
                let (response, mut session, mut msg_stream) = actix_ws::handle(&req, body).unwrap();

                async move {
                    let sid = uuid::Uuid::new_v4();
                    let mut res = io.input(sid, EngineInput::New(Some(config), crate::EngineKind::Continuous)).await;
                    let server_events_stream = io.input(sid, EngineInput::Listen(Participant::Server)).await;
                    let mut to_send = io.input(sid, EngineInput::Listen(Participant::Client)).await;

                   <F as NewConnectionService>::new_connection(
                       &client,
                       server_events_stream,
                        crate::io::AsyncSessionIOSender::new(sid,io.clone())
                   );

                    actix_rt::spawn(async move {
                        loop {
                            tokio::select! {
                                Some(Ok(start)) = res.next() => {
                                    dbg!();
                                    let p = start.as_bytes(sid);
                                    session.text(String::from_utf8(p).unwrap()).await;
                                },

                                ingress = msg_stream.next() => {
                                    let payload = match ingress {
                                        Some(Ok(m)) => {
                                            match m {
                                                Message::Ping(bytes) => Payload::Ping,
                                                Message::Pong(bytes) => Payload::Pong,
                                                // TODO: 
                                                Message::Text(s) => Payload::Message(s.to_string().as_bytes().to_vec()),
                                                Message::Binary(bytes) => Payload::Message(bytes.to_vec()),
                                                Message::Close(bytes) => break,
                                                Message::Continuation(bytes) => Payload::Ping,
                                                Message::Nop =>  Payload::Ping,
                                            }
                                        },
                                        _ => break
                                    };
                                    io.input(sid, EngineInput::Data(Participant::Client, payload)).await;
                                }

                                engress = to_send.next() => {
                                    match engress {
                                        Some(Ok(Payload::Message(d))) => session.text(String::from_utf8(d).unwrap()).await,
                                        Some(Ok(Payload::Close)) => break,
                                        Some(Err(e)) => { break},
                                        _ => Ok(())
                                        
                                    };
                                }
                            }
                        }
                        let _ = session.close(None).await;
                    });


                    response
                }
            }
            )
        )
    };

    // LONG POLL GET 
    let path = { 
        let io = io.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .guard(guard::fn_guard(|ctx| {
                ctx.head().uri.query().is_some_and(|s| s.contains("sid"))
            }))
            .to(move |session: web::Query<SessionInfo>| { 
                dbg!();
                let io = io.clone();
                async move {
                    if let Some(sid) = session.sid {
                        let res = io.input(sid, EngineInput::Poll(uuid::Uuid::new_v4())).await;
                        let (all, reason) = session_collect(res).await;
                        let mut response = if let Some(e) = reason {
                            match e {
                                EngineInputError::InvalidPoll => HttpResponse::BadRequest(),
                                _ => HttpResponse::InternalServerError(),
                            }
                        }
                        else { HttpResponse::Ok() };

                        let res_size = all.len();
                        let seperator = b"\x1e";
                        let combined: Vec<u8> = all.into_iter()
                            .map(|p| p.as_bytes(sid))
                            .enumerate()
                            .map(|(n,b)| if res_size > 1 && n < res_size - 1{ dbg!(&b); vec![b,seperator.to_vec()].concat() } else { b } )
                            .flat_map(|a| a )
                            .collect();

                        dbg!(&combined);
                        response.body(combined)
                    }
                    else {
                        HttpResponse::BadRequest().body(vec![])
                    }
                }
            })
        )
    };

    let path = {
        let io = io.clone();
        let client = client.clone();
        let config = config.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .to(move |session: web::Query<SessionInfo>| { 
                let io = io.clone();
                let client = client.clone();
                let config = config.clone();
                async move {
                    let sid = uuid::Uuid::new_v4();
                    let res = io.input(sid, EngineInput::New(Some(config), crate::EngineKind::Poll)).await;
                    let server_events_stream = io.input(sid, EngineInput::Listen(Participant::Server)).await;

                    // GET 
                    //
                    <F as NewConnectionService>::new_connection(
                        &client,
                        server_events_stream,
                        crate::io::AsyncSessionIOSender::new(sid,io)
                    );

                    let r = res.collect::<Vec<Result<Payload,EngineInputError>>>().await;
                
                    // TODO: WHY DOES LAST ERORR ?
                    match r.first() {
                        None => HttpResponse::InternalServerError().body(vec![]),
                        Some(Ok(p)) => HttpResponse::Ok().body(p.as_bytes(sid.clone())),
                        Some(Err(e)) => HttpResponse::BadRequest().body(vec![])
                    }

                }
            }
            )
        )
    };

    let path = { 
        let io = io.clone();
        path.route(
            web::route()
            .guard(guard::Post())
            .to(move |session: web::Query<SessionInfo>, body:web::Bytes| { 
                let io = io.clone();
                async move { 
                    let sid = session.sid.ok_or(EngineError::MissingSession)?;

                    let mut buf = vec![];
                    let mut start = 0;
                    let mut iter = body.iter().enumerate();
                    while let Some(d) = iter.next() {
                        let (n,data) = d;
                        if *data == b"\x1e"[0] {
                            buf.push(&body[start..n]);
                            start = n+1;
                            println!("test1b");
                        }
                    }
                     buf.push(&body[start..body.len()]);

                    for msg in buf.iter().filter(|v| v[0] == b"4"[0] ) {
                        let p = Payload::Message((**msg)[1..].to_vec());
                        io.input(sid, EngineInput::Data(Participant::Client, p)).await;
                    }
                    // TODO: Test suite assumes an "ok" returned in response... 
                    Ok::<HttpResponse, EngineError>(HttpResponse::Ok().body("ok"))
                }
            }
            )
        )
    };
    
    // Catch all - to appease clients expecting 400 not 404
    let path = {
        path.route(
            web::route()
            .to(move |session: web::Query<SessionInfo>| {
                HttpResponse::BadRequest()
            })
        )
    };
    return path
}

