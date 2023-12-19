use std::sync::Arc;

use actix_web::{guard, web, HttpRequest,  HttpResponse, Either, Responder, Resource};
use crate::*;
use futures_util::Stream;
use dashmap::DashMap;

use tokio::sync::mpsc;
use serde_json::json;

type PollChannelSenderMap = DashMap<Sid, mpsc::Sender<LongPollEvent>>;
type PollChannelReceiverMap = DashMap<Sid, tokio::sync::Mutex<mpsc::Receiver<Payload>>>;


pub use crate::io::AsyncEngineInner;
pub use crate::io::NewConnectionService;

#[cfg(feature = "actix")]
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


#[cfg(feature = "actix")]
struct WorkerState { 
    event_senders: DashMap<Sid, mpsc::Sender<LongPollEvent>>,
    engine_output: DashMap<Sid, tokio::sync::Mutex<mpsc::Receiver<Payload>>>,
}

#[cfg(feature = "actix")]
impl WorkerState {
    pub fn new() -> Self {
        return Self { event_senders:DashMap::new(), engine_output:DashMap::new() } 
    }
}

#[cfg(feature = "actix")]
#[derive(serde::Deserialize)]
struct SessionInfo {
    eio: u8,
    sid: Option<Sid> 
}

// THE EMITTER shouldn't care about transport 
// at the USER LEVEL its just a byte array 

// EMITTER takes ENGINE LEVEL events 
// SO ITS TX SENDER<PAYLOAD>

#[cfg(feature = "actix")]
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



#[cfg(feature = "actix")]
pub enum Emitter {
    WS(actix_ws::Session) ,
    POLL(mpsc::Sender<Payload>)
}

#[cfg(feature = "actix")]
impl Emitter {
    pub async fn emit(&mut self, msg:Vec<u8>) {
        match self {
            Self::POLL(rx) => rx.send(Payload::Message(msg)).await.unwrap(),
            Self::WS(session) => session.text("").await.unwrap()
        }
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
//where F: Fn(impl Stream) -> () + Clone + 'static,
//      S: Stream<Item = Vec<u8>>
where F: NewConnectionService + 'static
{

    //let path = {
    //    let callback = callback.clone();
    //    path.route(
    //        web::route()
    //        .guard(guard::Get())
    //        .guard(guard::fn_guard(|ctx| {
    //            ctx.head().headers().get("Upgrade").is_some_and(|v| v == "websocket")
    //        }))
    //        .to(move |req: HttpRequest, body: web::Payload| { 
    //            let callback = callback.clone();
    //            async move { create_sio_ws(req,body, callback).await }}
    //        )
    //    )
    //};


    // DashMap<Sid, mpsc::Sender<LongPollEvent>>
    let longpoll_senders: std::sync::Arc<PollChannelSenderMap> = DashMap::new().into();

    // DashMap<Sid, tokio::sync::Mutex<mpsc::Receiver<Payload>>>;
    let longpoll_readers: std::sync::Arc<PollChannelReceiverMap> = DashMap::new().into();

    let client = Arc::new(callback);

    // LONG POLL GET 
    let path = { 
        let longpoll_readers = longpoll_readers.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .guard(guard::fn_guard(|ctx| {
                ctx.head().uri.query().is_some_and(|s| s.contains("sid"))
            }))
            .to(move |session: web::Query<SessionInfo>| { 
                let longpoll_readers = longpoll_readers.clone();
                async move {
                    let res = poll_session(session.sid, &longpoll_readers).await?;
                    match res {
                        Payload::Message(m) => Ok::<web::Json<Vec<u8>>, SessionError>(web::Json(m)),
                        _ => Ok(web::Json(vec![]))
                    }
                }
            })
        )
    };

    let path = {
        let longpoll_readers = longpoll_readers.clone();
        let longpoll_senders = longpoll_senders.clone();
        //let callback = callback.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .to(move |session: web::Query<SessionInfo>| { 
                let longpoll_senders = longpoll_senders.clone();
                let longpoll_readers = longpoll_readers.clone();
               // let callback = callback.clone();
                let client = client.clone();

                let (sid,engine,tx) = create_session();
                let config = SessionConfig::default();
                longpoll_senders.insert(sid, tx);

                <F as NewConnectionService>::new_connection(&client, engine);
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
        let longpoll_senders = longpoll_senders.clone();
        path.route(
            web::route()
            .guard(guard::Post())
            .to(move |session: web::Query<SessionInfo>, body:web::Bytes| { 
                let longpoll_senders = longpoll_senders.clone();
                async move { 
                    post_session(session.sid, body.into(), &longpoll_senders).await?;
                    Ok::<HttpResponse, SessionError>(HttpResponse::Ok().finish())
                }
            }
            )
        )
    };
    return path
}
