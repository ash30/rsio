use std::sync::Arc;

use actix_web::{guard, web, HttpResponse, Resource, Route};
use tokio_stream::StreamExt;
use crate::*;
use dashmap::DashMap;
use serde_json::json;

use super::common::{LongPollRouter, SessionError, NewConnectionService};

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
        path.route(
            web::route()
            .guard(guard::Get())
            .guard(guard::fn_guard(|ctx| {
                ctx.head().headers().get("Upgrade").is_some_and(|v| v == "websocket")
            }))
            .to(move |req: actix_web::HttpRequest, body: web::Payload| { 
                let client = client.clone();
                let (response, session, mut msg_stream) = actix_ws::handle(&req, body).unwrap();
                tokio::spawn(async move {
                while let Some(Ok(msg)) = msg_stream.next().await {
                }

                });

                let (client_tx, client_rx) = tokio::sync::mpsc::channel(10);
                let (server_tx, server_rx) = tokio::sync::mpsc::channel(10);

                // NOTIFY CONSUMER
               // <F as NewConnectionService>::new_connection(
               //     &client,
               //     client_event_stream.filter_map(
               //         |p| if let Payload::Message(data) = p { Some(data) } else { None } 
               //     ),
               //     |data| { server_tx.send(LongPollEvent::POST(data)); }
               // );

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
                async move {
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
        path.route(
            web::route()
            .guard(guard::Get())
            .to(move |session: web::Query<SessionInfo>| { 
                let router = router.clone();
                let client = client.clone();

                let (client_tx, client_rx) = tokio::sync::mpsc::channel(10);
                let (server_tx, server_rx) = tokio::sync::mpsc::channel(10);
                let (sid, client_event_stream, server_event_rx) = create_poll_session(
                    tokio_stream::wrappers::ReceiverStream::new(client_rx),
                    tokio_stream::wrappers::ReceiverStream::new(server_rx)
                );        
                router.writers.insert(sid, client_tx);
                router.readers.insert(sid, server_event_rx.into());

                // NOTIFY CONSUMER
                <F as NewConnectionService>::new_connection(
                    &client,
                    client_event_stream.filter_map(
                        |p| if let Payload::Message(data) = p { Some(data) } else { None } 
                    ),
                    |data| { server_tx.send(LongPollEvent::POST(data)); }
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

