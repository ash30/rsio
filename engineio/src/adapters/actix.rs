use tokio_stream::StreamExt;
use actix_web::{guard, web, HttpResponse, Resource};
use crate::{create_async_io2, PayloadDecodeError, MessageData, EngineKind };
use crate::proto::Payload;
use crate::engine::{Sid, TransportConfig, EngineError, EngineIOClientCtrls, EngineInput };
pub use crate::AsyncConnectionService as ConnectionService;
pub use crate::AsyncSessionServerHandle as Session;

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
                Payload::decode(&data, EngineKind::Continuous)
            },
            actix_ws::Message::Binary(d) => {
                let data = d.into_iter().collect::<Vec<u8>>();
                return Ok(Payload::Message(crate::MessageData::Binary(data)))
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

pub fn socket_io<F>(path:actix_web::Resource, config:TransportConfig, callback: F) -> Resource
where F: ConnectionService + 'static + Send + Sync 
{
    let io = create_async_io2(callback, config);
    
    // WS 
    let path = {
        let io = io.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .guard(guard::fn_guard(|ctx| {
                ctx.head().headers().get("Upgrade").is_some_and(|v| v == "websocket")
            }))
            .to(move |req: actix_web::HttpRequest, body: web::Payload| { 
                let io = io.clone();
                let (response, mut session, mut msg_stream) = actix_ws::handle(&req, body).unwrap();
                async move {
                    let sid = uuid::Uuid::new_v4();
                    let mut client_stream = io.input_with_response_stream(
                        sid,
                        EngineInput::Control(EngineIOClientCtrls::New(EngineKind::Continuous))
                    ).await?;

                    actix_rt::spawn(async move {
                        while let Some(Ok(m)) = msg_stream.next().await {
                            let _ = io.input(sid, EngineInput::Data(m.try_into())).await;
                        }
                    });
                    actix_rt::spawn(async move {
                        while let Some(p) = client_stream.next().await {
                            let d = p.encode(EngineKind::Continuous);
                            let _ = match p {
                                Payload::Message(MessageData::Binary(..)) => session.binary(d).await,
                                _ => session.text(String::from_utf8(d).unwrap()).await
                            };
                        }
                        let _ = session.close(None).await;
                    });
                    return Ok::<HttpResponse, EngineError>(response);
                }
            })
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
                let io = io.clone();
                async move {
                    let sid = session.sid.ok_or(EngineError::MissingSession)?;
                    let s = io.input_with_response_stream(sid, EngineInput::Control(EngineIOClientCtrls::Poll)).await?;
                    let all = s.collect::<Vec<Payload>>().await;
                    Ok::<HttpResponse,EngineError>(HttpResponse::Ok().body(Payload::encode_combined(&all, EngineKind::Poll)))
                }
            })
        )
    };

    // NEW LONG POLL
    let path = {
        let io = io.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .to(move | _session: web::Query<SessionInfo>| { 
                let io = io.clone();
                async move {
                    let sid = uuid::Uuid::new_v4();
                    let s = io.input_with_response_stream(sid, EngineInput::Control(EngineIOClientCtrls::New(crate::EngineKind::Poll))).await?;
                    let all = s.take(1).collect::<Vec<Payload>>().await;
                    let combined = Payload::encode_combined(&all, EngineKind::Poll);
                    Ok::<HttpResponse,EngineError>(HttpResponse::Ok().body(combined))
                }
            }
            )
        )
    };

    // POST MSG LONG POLL
    let path = { 
        let io = io.clone();
        path.route(
            web::route()
            .guard(guard::Post())
            .to(move |session: web::Query<SessionInfo>, body:web::Bytes| { 
                let io = io.clone();
                async move { 
                    let sid = session.sid.ok_or(EngineError::MissingSession)?;
                    for p in Payload::decode_combined(body.as_ref(), EngineKind::Poll) {
                        if let Err(e) = io.input(sid, EngineInput::Data(p)).await {
                            return Err(e);
                        }
                    }
                    Ok(Ok::<HttpResponse, EngineError>(HttpResponse::Ok().body("ok")))
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

