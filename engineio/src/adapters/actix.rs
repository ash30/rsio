use std::sync::Arc;
use tokio_stream::StreamExt;
use actix_web::{guard, web, HttpResponse, Resource};
use crate::{async_engine_create, EngineInput, PayloadDecodeError, MessageData, EngineKind };
use crate::engine::{Sid, TransportConfig, Payload, Participant, EngineError };
pub use super::common::{ NewConnectionService, AsyncEmitter as Emitter };
use super::common::create_connection_stream;

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
                let t = EngineKind::Continuous;
                Payload::decode(&data, &t)
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
            EngineError::InvalidPoll => actix_web::http::StatusCode::BAD_REQUEST,
            EngineError::Generic => actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
            EngineError::MissingSession => actix_web::http::StatusCode::BAD_REQUEST,
            _ => actix_web::http::StatusCode::NOT_FOUND
        }
    }

    fn error_response(&self) -> HttpResponse<actix_web::body::BoxBody> {
        return HttpResponse::new(self.status_code())
    }

}

pub fn socket_io<F>(path:actix_web::Resource, config:TransportConfig, callback: F) -> Resource
where F: NewConnectionService + 'static
{
    let client = Arc::new(callback);
    let io = async_engine_create();

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
                    let mut client_stream = io.input(
                        sid, EngineInput::New(Some(config), crate::EngineKind::Continuous)
                    ).await.map_err(|_e| EngineError::OpenFailed)?.unwrap();

                    let server_stream = io.input(
                        sid, EngineInput::Listen
                    ).await.map_err(|_e| EngineError::OpenFailed)?.unwrap();

                   <F as NewConnectionService>::new_connection(
                        &client,
                        create_connection_stream(server_stream),
                        Emitter::new(sid, io.clone())
                   );

                    actix_rt::spawn(async move {
                        loop {
                            tokio::select! {
                                ingress = msg_stream.next() => {
                                    match ingress {
                                        Some(Ok(m)) => io.input(sid, EngineInput::Data(Participant::Client, m.try_into())).await,
                                        _ => break
                                    };
                                }
                                engress = client_stream.next() => {
                                    match &engress {
                                        Some(p) => { 
                                            let t = EngineKind::Continuous;
                                            let d = p.encode(&t);
                                            match p {
                                                Payload::Message(MessageData::Binary(..)) => session.binary(d).await,
                                                _ => session.text(String::from_utf8(d).unwrap()).await
                                            }   
                                        },
                                        None => break
                                    };
                                }
                            }
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
                dbg!();
                let io = io.clone();
                async move {
                    if let Some(sid) = session.sid {
                        let s = io.input(sid, EngineInput::Poll).await;
                        match s {
                            Err(..) =>  HttpResponse::BadRequest().body(""),
                            Ok(None) => HttpResponse::InternalServerError().body(""),
                            Ok(Some(s)) => {
                                let all = s.collect::<Vec<Payload>>().await;
                                let t = EngineKind::Poll;
                                let combined = Payload::encode_combined(&all, &t);
                                dbg!(&combined);
                                HttpResponse::Ok().body(combined)
                            }
                        }
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
                    match res {
                        Err(e) => {  dbg!(e); dbg!(HttpResponse::BadRequest().body(""))},
                        Ok(None) => dbg!(HttpResponse::BadRequest().body("")),
                        Ok(Some(s)) => {
                            if let Ok(Some(server_stream)) = io.input(sid, EngineInput::Listen).await {
                                <F as NewConnectionService>::new_connection(
                                    &client,
                                    create_connection_stream(server_stream),
                                    Emitter::new(sid, io.clone())
                                );
                            }

                            let all = s.take(1).collect::<Vec<Payload>>().await;
                            let t = EngineKind::Poll;
                            let combined = Payload::encode_combined(&all, &t);
                            HttpResponse::Ok().body(combined)
                        }
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
                    let t = EngineKind::Poll;
                    for p in Payload::decode_combined(body.as_ref(), &t) {
                        dbg!(&p);
                        io.input(sid, EngineInput::Data(Participant::Client, p)).await;
                    }
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

