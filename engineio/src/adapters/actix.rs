use actix_web::{guard, web, HttpRequest,  HttpResponse, Either, Responder, Resource};
use crate::*;
use futures_util::Stream;
use dashmap::DashMap;

use tokio::sync::mpsc;
use serde_json::json;

#[cfg(feature = "actix")]
impl From<actix_ws::Message> for WebsocketEvent {
    fn from(value: actix_ws::Message) -> Self {
        Self::Ping
    }
}

#[cfg(feature = "actix")]
impl From<Result<actix_ws::Message, actix_ws::ProtocolError>> for WebsocketEvent {
    fn from(value: Result<actix_ws::Message, actix_ws::ProtocolError>) -> Self {
        Self::Ping
    }
}

#[cfg(feature = "actix")]
enum Error  { }


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

#[cfg(feature = "actix")]
pub enum AsyncEngine {
    WS(AsyncEngineInner<Websocket, actix_ws::MessageStream>),
    POLL(AsyncEngineInner<LongPoll, tokio_stream::wrappers::ReceiverStream<LongPollEvent>>)

}

impl Stream for AsyncEngine 
{
    type Item = Vec<u8>;
    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
        match self.get_mut() {
            AsyncEngine::WS(e) => std::pin::Pin::new(e).poll_next(cx),
            AsyncEngine::POLL(e) => std::pin::Pin::new(e).poll_next(cx),
        }
    }
}

async fn create_sio_ws<F>(req: HttpRequest, body: web::Payload, handler:F)-> impl Responder 
where F:  Fn(AsyncEngine, Emitter) -> (),
{
    let engine = Engine::new_ws();
    let (response, session, msg_stream) = actix_ws::handle(&req, body).unwrap();

    let config = SessionConfig::default();
    let res = json!({
      "sid": engine.session,
      "upgrades": [],
      "pingInterval": config.ping_interval,
      "pingTimeout": config.ping_timeout,
      "maxPayload": config.max_payload
    });

    let emitter = Emitter::WS(session);
    let sio = AsyncEngine::WS(AsyncEngineInner::new(engine, msg_stream));
    handler(sio,emitter);

    return web::Bytes::from(res.to_string());
}

async fn create_sio_poll<F>(query: web::Query<SessionInfo>, state:std::sync::Arc<WorkerState>, handler:F)-> Either<HttpResponse,web::Bytes> 
where F:  Fn(AsyncEngine, Emitter) -> (),
{
    let engine = Engine::new_longpoll();
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    state.event_senders.insert(engine.session,tx);

    let (etx, erx) = tokio::sync::mpsc::channel(10);
    state.engine_output.insert(engine.session, erx.into());

    let config = SessionConfig::default();
    let res = json!({
      "sid": engine.session,
      "upgrades": ["websocket"],
      "pingInterval": config.ping_interval,
      "pingTimeout": config.ping_timeout,
      "maxPayload": config.max_payload
    });

    let sio = AsyncEngine::POLL(AsyncEngineInner::new(engine, rx.into()));
    let s = Emitter::POLL(etx.clone());

    handler(sio,s);

    return Either::Right(web::Bytes::from(res.to_string()))
}
    
async fn post_sio(query: web::Query<SessionInfo>, state:std::sync::Arc<WorkerState>, body: web::Bytes) -> HttpResponse {
    match query.sid.as_ref().and_then(|sid| state.event_senders.get(sid)){
       None => HttpResponse::NotFound().finish(),
       Some(tx) => {
            tx.send(LongPollEvent::POST(body.into())).await;
            HttpResponse::Ok().finish()
       }
    }
}

async fn get_sio(query: web::Query<SessionInfo>, state:std::sync::Arc<WorkerState>) -> impl Responder {
    match state.engine_output.get_mut(&query.sid.unwrap()) {
       Some(rx) => {
           match rx.try_lock() {
               Ok(mut rx) => {
                   match rx.recv().await {
                       None => Either::Left(HttpResponse::Ok().finish()),
                       //Some(out) => Either::Right(bytes)
                       Some(out) => { 
                            match out { 
                                Payload::Message(m) => Either::Right(web::Json(m)),
                                _ => Either::Left(HttpResponse::Ok().finish())
                            }
                       }
                   }
               }
               Err(e) => Either::Left(HttpResponse::BadRequest().finish()) // SHOULD CLOSE SES
           }
       }
       None => return Either::Left(HttpResponse::NotFound().finish())
   }
}

pub fn socket_io<F>(path:&str, callback: F) -> Resource
where F: Fn(AsyncEngine, Emitter) -> () + Clone + 'static
{
    // Private state for service
    let state = std::sync::Arc::new(WorkerState::new());
    let s1 = state.clone();
    let s2 = state.clone();
    let s3 = state.clone();

    let c1 = callback.clone();
    let c2 = callback.clone();

    web::resource(path)
    .route(
        web::route()
        .guard(guard::Get())
        .guard(guard::fn_guard(|ctx| {
            ctx.head().uri.query().is_some_and(|s| s.contains("sid"))
        }))
        .to(move |session: web::Query<SessionInfo>| { 
            let s = s1.clone();
            async move { get_sio(session, s.clone()).await }}
        )
    )
    .route(
        web::route()
        .guard(guard::Get())
        .guard(guard::fn_guard(|ctx| {
            ctx.head().headers().get("Upgrade").is_some_and(|v| v == "websocket")
        }))
        .to(move |req: HttpRequest, body: web::Payload| { 
            let f = c1.clone();
            async move { create_sio_ws(req,body, f).await }}
        )
    )
    .route(
        web::route()
        .guard(guard::Get())
        .to(move |session: web::Query<SessionInfo>| { 
            let s = s2.clone();
            let f = c2.clone();
            async move { create_sio_poll(session, s.clone(), f).await }}
        )
    )
    .route(
        web::route()
        .guard(guard::Post())
        .to(move |session: web::Query<SessionInfo>, body:web::Bytes| { 
            let s = s3.clone();
            async move { post_sio(session, s.clone(), body).await }}
        )
    )
}
