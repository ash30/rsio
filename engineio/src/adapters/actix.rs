use tokio::time::Instant;
use tokio_stream::StreamExt;
use actix_web::{guard, web, HttpResponse, Resource, Either, ResponseError};
use crate::io::{self, SessionCloseReason, create_session_local};
use crate::proto::{Sid, Payload, PayloadDecodeError, MessageData, TransportConfig};
use crate::server::EngineIOServer;
use crate::transport::{TransportKind};
use crate::engine::{EngineError, self, Engine, EngineIOClientCtrls, EngineInput};


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

fn engine_io(path:actix_web::Resource, config:TransportConfig) -> Resource {
    let polling = io::create_multiplex();

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
                    let engine = Engine::new(EngineIOServer::new(sid,Instant::now().into()));
                    let session = create_session_local(engine, |tx,rx| {
                        async move {
                            loop {
                                tokio::select! {
                                    recv = msg_stream.next() => {
                                        let Some(r) = recv else { break };
                                        let res = tokio::sync::oneshot::channel();
                                        let input  = match r {
                                            Ok(m) => EngineInput::Data(m.try_into()),
                                            Err(e) => EngineInput::Control(EngineIOClientCtrls::Close)
                                        };
                                        let s = tx.send((input,res.0)).await;
                                    },
                                    send = rx.recv() => {
                                        let Some(s) = send else { break };
                                        let d = s.encode(TransportKind::Continuous);
                                        let _ = match s {
                                            Payload::Message(MessageData::Binary(..)) => session.binary(d).await,
                                            _ => session.text(String::from_utf8(d).unwrap()).await
                                        };
                                    }
                                } 
                            }
                            //TODO 
                            SessionCloseReason::Unknown
                        }
                    } );
                    return Ok::<HttpResponse, EngineError>(response);
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
                    let session = polling.create(sid).await;
                    // DO something with session
                    match polling.listen(sid).await {
                        Ok(vec) => HttpResponse::Ok().body(Payload::encode_combined(&vec, TransportKind::Poll)),
                        Err(e) => e.error_response()
                    }
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
                    if let Err(err) = polling.input(sid, engine::EngineInput::Control(EngineIOClientCtrls::Poll)).await {
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
                    HttpResponse::Ok().finish()
                }
            }
            )
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


