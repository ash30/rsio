use actix_ws::CloseReason;
use tokio::time::Instant;
use actix_web::{guard, web, HttpResponse, Resource, ResponseError};
use tokio_stream::StreamExt;
use crate::io::{self, SessionCloseReason, create_session_local, Session};
use crate::proto::{Sid, Payload, PayloadDecodeError, MessageData };
use crate::transport::TransportKind;
use crate::engine::{EngineError, self, Engine, EngineInput, EngineSignal, EngineCloseReason};

pub use crate::proto::TransportConfig;
pub type IOEngine = Session;

#[derive(serde::Deserialize)]
struct SessionInfo {
    #[serde(alias = "EIO")]
    eio: u8,
    sid: Option<Sid> 
}

impl TryFrom<actix_ws::Message> for Payload {
    type Error = PayloadDecodeError;

    fn try_from(value: actix_ws::Message) -> Result<Self, Self::Error> {
        match value {
            actix_ws::Message::Text(d) => {
                let data = d.as_bytes().to_vec();
                Payload::decode(&data, TransportKind::Continuous)
            },
            actix_ws::Message::Binary(d) => {
                let data = d.into_iter().collect::<Vec<u8>>();
                return Ok(Payload::Message(MessageData::Binary(data)))
            },
            _ => Ok(Payload::Noop)
        }
    }
}

impl actix_web::ResponseError for EngineError {
    fn status_code(&self) -> actix_web::http::StatusCode {
        match self {
            EngineError::Generic => actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
            _ => actix_web::http::StatusCode::BAD_REQUEST
        }
    }

    fn error_response(&self) -> HttpResponse<actix_web::body::BoxBody> {
        return HttpResponse::new(self.status_code())
    }

}

pub fn engine_io(path:actix_web::Resource, config:TransportConfig, service:fn(IOEngine)) -> Resource {
    let polling = io::create_multiplex(config.clone());

    let path = { 
        path.route(
            web::route()
            .guard(guard::Get())
            .guard(guard::fn_guard(|ctx| {
                ctx.head().headers().get("Upgrade").is_some_and(|v| v == "websocket")
            }))
            .to(move |req: actix_web::HttpRequest, body: web::Payload| { 
                let (response, mut session, mut msg_stream) = actix_ws::handle(&req, body).unwrap();
                async move {
                    let sid = uuid::Uuid::new_v4();
                    let mut engine = Engine::new(Instant::now().into(), GenericTransport{});
                    engine.recv(EngineInput::Control(EngineSignal::New(TransportKind::Continuous)), Instant::now().into(), &config);

                    let session = create_session_local(engine, config.clone(), |tx,mut rx| {
                        async move {
                            let reason = loop {
                                tokio::select! {
                                    recv = msg_stream.next() => {
                                        dbg!(&recv);
                                        let Some(r) = recv else { break SessionCloseReason::TransportClose};
                                        let res = tokio::sync::oneshot::channel();
                                        let input  = match r {
                                            Ok(m) => EngineInput::Data(m.try_into()),
                                            Err(_) => EngineInput::Data(Err(PayloadDecodeError::InvalidFormat))
                                        };
                                        dbg!(&input);
                                        if let Err(_) =  tx.send((input,res.0)).await {
                                            break SessionCloseReason::Unknown;
                                        }
                                        if let Err(_) = res.1.await {
                                            break SessionCloseReason::Unknown;
                                        }
                                    },
                                    send = rx.recv() => {
                                        let Some(s) = send else { break SessionCloseReason::Unknown } ;
                                        let d = s.encode(TransportKind::Continuous);
                                        let res = match s {
                                            Payload::Message(MessageData::Binary(..)) => session.binary(d).await,
                                            _ => session.text(String::from_utf8(d).unwrap()).await
                                        };
                                        if let Err(_) = res { break SessionCloseReason::TransportClose } 
                                    }
                                } 
                            };
                            // close transport out once engine has closed 
                            tokio::spawn(session.close(Some(CloseReason { code: actix_ws::CloseCode::Normal, description: Option::None })));
                            // return reason so can propagate up to session owner
                            return reason
                        }
                    } );
                    service(session);
                    return Ok::<HttpResponse, EngineError>(response);
                }
            })
        )
    };

    // LONG POLL GET 
    let path = { 
        let polling = polling.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .guard(guard::fn_guard(|ctx| {
                ctx.head().uri.query().is_some_and(|s| s.contains("sid"))
            }))
            .to(move |session: web::Query<SessionInfo>| { 
                let polling = polling.clone();
                async move {
                    let Some(sid) = session.sid else {
                        return EngineError::MissingSession.error_response()
                    };
                    if let Err(_) = polling.input(sid, engine::EngineInput::Control(EngineSignal::Poll)).await {
                        return (EngineError::MissingSession).error_response()
                    }
                    match polling.listen(sid).await {
                        Ok(vec) => HttpResponse::Ok().body(Payload::encode_combined(&vec, TransportKind::Poll)),
                        Err(e) => e.error_response()
                    }
                }
            })
        )
    };

    // LONG POLL CREATE
    let path = {
        let polling = polling.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .to(move | _session: web::Query<SessionInfo>| {
                let polling = polling.clone();
                async move {
                    let sid = Sid::new_v4();
                    let Ok(session) = polling.create(sid).await else {
                        return HttpResponse::InternalServerError().finish();
                    };
                    service(session);
                    match polling.listen(sid).await {
                        Ok(vec) => HttpResponse::Ok().body(Payload::encode_combined(&vec, TransportKind::Poll)),
                        Err(e) => e.error_response()
                    }
                }
            })
        )
    };

    // POST MSG LONG POLL
    let path = { 
        let polling = polling.clone();
        path.route(
            web::route()
            .guard(guard::Post())
            .to(move |session: web::Query<SessionInfo>, body:web::Bytes| { 
                let polling = polling.clone();
                async move { 
                    let Some(sid) = session.sid else {
                        return EngineError::MissingSession.error_response()
                    };
                    for p in Payload::decode_combined(body.as_ref(), TransportKind::Poll) {
                        // TODO: Handle errors please
                        polling.input(sid, EngineInput::Data(p)).await;
                    }
                    HttpResponse::Ok().body("ok")
                }
            }
            )
        )
    };

    let path = {
        let polling = polling.clone();
        path.route(
            web::route()
            .guard(guard::Delete())
            .to(move | session:web::Query<SessionInfo>| {
                let polling = polling.clone();
                async move { 
                    let Some(sid) = session.sid else {
                        return EngineError::MissingSession.error_response()
                    };
                    if let Err(_) = polling.input(sid, EngineInput::Data(Ok(Payload::Close(EngineCloseReason::ClientClose)))).await {
                        return (EngineError::MissingSession).error_response()
                    }
                    match polling.delete(sid).await {
                        Ok(_) => HttpResponse::Ok().finish(),
                        Err(e) => e.error_response()
                    }
                }
            })
        )
    };
    
    // Catch all - to appease clients expecting 400 not 404
    let path = {
        path.route(
            web::route()
            .to(move | _session: web::Query<SessionInfo>| {
                HttpResponse::BadRequest()
            })
        )
    };

    return path
}


