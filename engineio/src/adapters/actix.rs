use std::sync::Arc;
use tokio_stream::StreamExt;
use serde_json::json;
use actix_web::{guard, web, HttpResponse, Resource};
use actix_ws::Message;
use tokio::sync::broadcast::error::RecvError;

use crate::EngineOutput;
use crate::engine::{Sid, WebsocketEvent, TransportConfig, Payload, Participant };
use crate::io::create_session_async;
use crate::proto::EngineError;
use crate::proto::EngineInput;
use super::common::LongPollRouter;
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
            EngineError::Generic => actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
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
                let (response, mut session, mut msg_stream) = actix_ws::handle(&req, body).unwrap();

                let mut engine = crate::engine::Engine::new();
                engine.consume(EngineInput::New(Some(TransportConfig::default())), std::time::Instant::now());

                let (sid,io) = create_session_async(engine);
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

                                if let Err(_e) = client_tx.send(EngineInput::Data(Participant::Client,payload)).await {
                                    break
                                }
                            }

                            engress = server_rx.recv() => {
                                match engress {
                                    Ok(Payload::Close(..)) => {
                                        break
                                    },

                                    Ok(Payload::Message(p)) => {
                                        session.text(std::str::from_utf8(&p).unwrap()).await;
                                    },

                                    Ok(Payload::Ping) => {
                                        session.ping(b"").await;
                                    },

                                    Ok(Payload::Pong) => {
                                        session.pong(b"").await;
                                    },

                                    Ok(Payload::Open(data)) => {
                                        // TODO:
                                    },

                                    Ok(Payload::Noop) => {
                                    },

                                    Ok(Payload::Upgrade) => {

                                    },

                                    Err(RecvError::Closed) => {

                                    },

                                    Err(RecvError::Lagged(_)) => {

                                    }
                                }
                            }
                        }
                    }
                    let _ = session.close(None).await;
                    drop(server_rx);
                    drop(client_tx);
                });

                <F as NewConnectionService>::new_connection(
                    &client,
                    tokio_stream::wrappers::BroadcastStream::new(client_rx),
                    Emitter { tx: server_tx }
                );

                let config = TransportConfig::default();
                async move { 
                    let res = json!({
                      "sid": sid,
                      "upgrades": ["websocket"],
                      "pingInterval": config.ping_interval,
                      "pingTimeout": config.ping_timeout,
                      "maxPayload": config.max_payload
                    });
                    
                    response
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
                async move {
                    let sid = session.sid.ok_or(EngineError::UnknownSession)?;
                    let res = router.poll(session.sid).await?;
                    let seperator = "\x1e";
                    let b:Vec<u8>= res.iter().map(|p| vec![p.as_bytes(sid), seperator.as_bytes().to_owned() ].concat()).fold(vec![], |a,b| {
                        a.into_iter()
                            .chain(b.into_iter()).collect()
                    });
                    Ok::<Vec<u8>,EngineError>(b)
                }
            })
        )
    };

    let path = {
        let router = longpoll.clone();
        let client = client.clone();
        let config = config.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .to(move |session: web::Query<SessionInfo>| { 
                let router = router.clone();
                let client = client.clone();
                let config = config.clone();
                async move {
                    let mut engine = crate::engine::Engine::new();
                    engine.consume(EngineInput::New(Some(config)), std::time::Instant::now());

                    let res = loop {
                        match engine.poll_output() {
                            EngineOutput::Data(Participant::Server, payload) => {
                                if let Payload::Open(..) = payload { break Some(payload) } 
                            }
                            _ => {}
                        };
                        break None;
                    }.ok_or(EngineError::Generic)?;

                    let (sid,io) = create_session_async(engine);
                    let (client_tx, client_rx) = io.0; 
                    let (server_tx, server_rx) = io.1;

                    router.writers.insert(sid, client_tx);
                    router.readers.insert(sid, server_rx.into());

                    <F as NewConnectionService>::new_connection(
                        &client,
                        tokio_stream::wrappers::BroadcastStream::new(client_rx),
                        Emitter { tx: server_tx }


                    );
                    Ok::<actix_web::web::Bytes, EngineError>(web::Bytes::from(res.as_bytes(sid)))
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
                let router = router.clone();
                async move { 
                    router.post(session.sid, body.into()).await?;
                    Ok::<HttpResponse, EngineError>(HttpResponse::Ok().finish())
                }
            }
            )
        )
    };
    return path
}

