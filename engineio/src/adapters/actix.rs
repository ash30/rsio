use std::sync::Arc;
use tokio_stream::StreamExt;
use actix_web::{guard, web, HttpResponse, Resource};

use crate::{async_session_io_create, EngineInput, PayloadDecodeError, MessageData, EngineKind };
use crate::engine::{Sid, TransportConfig, Payload, Participant, EngineError };
pub use super::common::{ NewConnectionService, Emitter };

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

#[derive(serde::Deserialize)]
struct SessionInfo {
    #[serde(alias = "EIO")]
    eio: u8,
    sid: Option<Sid> 
}

impl actix_web::ResponseError for EngineError{
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
                    let s = io.input(sid, EngineInput::New(Some(config), crate::EngineKind::Continuous)).await;
                    let s2 = io.input(sid, EngineInput::Listen).await;

                    match (s2,s) {
                        (Ok(Some(server_stream)), Ok(Some(mut client_stream))) => {
                           <F as NewConnectionService>::new_connection(
                               &client,
                               server_stream.filter_map(|p| match p { 
                                    Payload::Message(d) => Some(Ok(d)),
                                    Payload::Close(r) => Some(Err(r)),
                                    _ => None,
                               }),
                                crate::io::AsyncSessionIOSender::new(sid,io.clone())
                           );
                            //std::pin::pin!(client_stream);
                            //std::pin::pin!(msg_stream);
                            actix_rt::spawn(async move {
                                loop {
                                    tokio::select! {
                                        ingress = msg_stream.next() => {
                                            let payload = match ingress {
                                                Some(Ok(m)) => m.try_into(),
                                                _ => break
                                            };
                                            io.input(sid, EngineInput::Data(Participant::Client, payload)).await;
                                        }
                                        engress = client_stream.next() => {
                                            match &engress {
                                                Some(p) => { 
                                                    dbg!(&p);

                                                    let d = p.encode(EngineKind::Continuous);
                                                    match p {
                                                        Payload::Message(MessageData::Binary(..)) => {
                                                            dbg!(session.binary(d).await);
                                                        },
                                                        _ => {
                                                            dbg!(session.text(String::from_utf8(d).unwrap()).await);

                                                        }
                                                    }   
                                                },
                                                None => {
                                                    dbg!();
                                                    break
                                                }
                                            };
                                        }
                                    }
                                }
                                dbg!();
                                let _ = session.close(None).await;
                            });
                        },
                        _ => {}
                    };
                    return response
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
                        let s = io.input(sid, EngineInput::Poll).await;
                        match s {
                            Err(e) =>  HttpResponse::BadRequest().body(""),
                            Ok(None) => HttpResponse::InternalServerError().body(""),
                            Ok(Some(s)) => {
                                let all = s.collect::<Vec<Payload>>().await;

                                let res_size = all.len();
                                let seperator = b"\x1e";
                                let combined: Vec<u8> = all.into_iter()
                                    .map(|p| p.encode(EngineKind::Poll).to_owned())
                                    .enumerate()
                                    .map(|(n,b)| if res_size > 1 && n < res_size - 1{ dbg!(&b); vec![b,seperator.to_vec()].concat() } else { b } )
                                    .flat_map(|a| a )
                                    .collect();

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

                    dbg!();
                    match res {
                        Err(e) => {  dbg!(e); dbg!(HttpResponse::BadRequest().body(""))},
                        Ok(None) => dbg!(HttpResponse::BadRequest().body("")),
                        Ok(Some(s)) => {


                            dbg!();
                            if let Ok(Some(server_stream)) = io.input(sid, EngineInput::Listen).await {
                                dbg!();
                                <F as NewConnectionService>::new_connection(
                                    &client,
                                    server_stream.filter_map(|p| match p { 
                                         Payload::Message(d) => Some(Ok(d)),
                                         Payload::Close(r) => Some(Err(r)),
                                         _ => None,
                                    }),
                                    crate::io::AsyncSessionIOSender::new(sid,io)
                                );
                            }

                            let all = s.take(1).collect::<Vec<Payload>>().await;
                            let res_size = all.len();
                            let seperator = b"\x1e";
                            let combined: Vec<u8> = all.into_iter()
                                .map(|p| p.encode(EngineKind::Poll).to_owned())
                                .enumerate()
                                .map(|(n,b)| if res_size > 1 && n < res_size - 1{ dbg!(&b); vec![b,seperator.to_vec()].concat() } else { b } )
                                .flat_map(|a| a )
                                .collect();

                            dbg!(&combined);

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

                    let mut buf = vec![];
                    let mut start = 0;
                    let mut iter = body.iter().enumerate();
                    while let Some(d) = iter.next() {
                        let (n,data) = d;
                        if *data == b"\x1e"[0] {
                            buf.push(&body[start..n]);
                            start = n+1;
                        }
                    }
                     buf.push(&body[start..body.len()]);

                    for msg in buf.into_iter() {
                        dbg!(msg);
                        io.input(sid, EngineInput::Data(Participant::Client, Payload::decode(msg, EngineKind::Poll))).await;
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

