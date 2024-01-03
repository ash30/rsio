use std::sync::Arc;
use tokio_stream::StreamExt;
use serde_json::json;
use actix_web::{guard, web, HttpResponse, Resource};
use actix_ws::Message;

use crate::engine::{Sid, WebsocketEvent, SessionConfig, Payload };
use crate::io::create_session;
use super::common::{ LongPollRouter, SessionError };

pub use super::common::NewConnectionService;
pub use super::common::Emitter;

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
    eio: u8,
    sid: Option<Sid> 
}

impl actix_web::ResponseError for SessionError{
    fn status_code(&self) -> actix_web::http::StatusCode {
        match self {
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
pub fn socket_io<F>(path:actix_web::Resource, callback: F) -> Resource
where F: NewConnectionService + 'static
{
    let client = Arc::new(callback);

    // WS 
    let path = {
        let client = client.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .guard(guard::fn_guard(|ctx| {
                ctx.head().headers().get("Upgrade").is_some_and(|v| v == "websocket")
            }))
            .to(move |req: actix_web::HttpRequest, body: web::Payload| { 
                let client = client.clone();
                let (response, session, mut msg_stream) = actix_ws::handle(&req, body).unwrap();

                println!("baz 1");


                let (sid,io) = create_session();
                let (client_tx, client_rx) = io.0; 
                let (server_tx, mut server_rx) = io.1;

                actix_rt::spawn(async move {
                    loop {
                        tokio::select! {
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
                                            _ => break,
                                        }
                                    },
                                    _ => break
                                };

                                if let Err(_e) = client_tx.send(payload).await {
                                    break
                                }
                            }

                            engress = server_rx.recv() => {
                                match engress {
                                    Some(p) => {
                                        match p {
                                            Payload::Close => break,
                                            _ => {}
                                        }
                                    },
                                    None =>  {}
                                }
                            }
                        }
                    }
                    let _ = session.close(None).await;
                    server_rx.close();
                    drop(client_tx);
                });

                <F as NewConnectionService>::new_connection(
                    &client,
                    tokio_stream::wrappers::ReceiverStream::new(client_rx)
                    .filter_map(
                        |p| if let Payload::Message(data) = p { Some(data) } else { None } 
                    ),
                    Emitter { tx: server_tx }
                );

                let config = SessionConfig::default();
                async move { 
                    let res = json!({
                      "sid": sid,
                      "upgrades": ["websocket"],
                      "pingInterval": config.ping_interval,
                      "pingTimeout": config.ping_timeout,
                      "maxPayload": config.max_payload
                    });
                    web::Bytes::from(res.to_string()) 
                }
            }
            )
        )
    };
    let longpoll = Arc::new(LongPollRouter::new());

    // LONG POLL GET 
    let path = { 
        let router = longpoll.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .guard(guard::fn_guard(|ctx| {
                ctx.head().uri.query().is_some_and(|s| s.contains("sid"))
            }))
            .to(move |session: web::Query<SessionInfo>| { 
                let router = router.clone();
                println!("Spam 1");
                async move {
                    println!("SPam 2");
                    let res = router.poll_session(session.sid).await?;
                    match res {
                        Payload::Message(m) => Ok::<web::Json<Vec<u8>>, SessionError>(web::Json(m)),
                        _ => Ok(web::Json(vec![]))
                    }
                }
            })
        )
    };

    let path = {
        let router = longpoll.clone();
        let client = client.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .to(move |session: web::Query<SessionInfo>| { 
                println!("here 1");

                let router = router.clone();
                let client = client.clone();

                let (sid,io) = create_session();
                let (client_tx, client_rx) = io.0; 
                let (server_tx, server_rx) = io.1;
                
                router.writers.insert(sid, client_tx);
                router.readers.insert(sid, server_rx.into());

                // NOTIFY CONSUMER
                <F as NewConnectionService>::new_connection(
                    &client,
                    tokio_stream::wrappers::ReceiverStream::new(client_rx)
                    .filter_map(
                        |p| if let Payload::Message(data) = p { Some(data) } else { None } 
                    ),
                    Emitter { tx: server_tx }
                );

                let config = SessionConfig::default();
                async move { 
                    let res = json!({
                      "sid": sid,
                      "upgrades": ["websocket"],
                      "pingInterval": config.ping_interval,
                      "pingTimeout": config.ping_timeout,
                      "maxPayload": config.max_payload
                    });
                    web::Bytes::from(res.to_string()) 
                }
            }
            )
        )
    };

    let path = { 
        let router = longpoll.clone();
        path.route(
            web::route()
            .guard(guard::Post())
            .to(move |session: web::Query<SessionInfo>, body:web::Bytes| { 

                println!("foo 1");
                let router = router.clone();
                async move { 
                    router.post_session(session.sid, body.into()).await?;
                    Ok::<HttpResponse, SessionError>(HttpResponse::Ok().finish())
                }
            }
            )
        )
    };
    return path
}

