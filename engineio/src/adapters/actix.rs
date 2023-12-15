use actix_web::{guard, web, HttpRequest,  HttpResponse, Either, Responder, Resource};
use crate::*;
use futures_util::Stream;
use dashmap::DashMap;

use tokio::sync::mpsc;
use serde_json::json;

type PollChannelSenderMap = DashMap<Sid, mpsc::Sender<LongPollEvent>>;
type PollChannelReceiverMap = DashMap<Sid, tokio::sync::Mutex<mpsc::Receiver<Payload>>>;

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

// THE EMITTER shouldn't care about transport 
// at the USER LEVEL its just a byte array 

// EMITTER takes ENGINE LEVEL events 
// SO ITS TX SENDER<PAYLOAD>

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


async fn create_sio_poll<F>(query: web::Query<SessionInfo>, readers: &PollChannelReceiverMap, writers: &PollChannelSenderMap, handler:F)-> Either<HttpResponse,web::Bytes> 
where F:  Fn(AsyncEngine, Emitter) -> (),
{
    // THIS CHANNEL IS FOR CLIENT EVENTS, we buffer them and let engine stream consume 
    let engine = Engine::new_longpoll();
    let (tx, rx) = tokio::sync::mpsc::channel(10);

    writers.insert(engine.session, tx.clone());
    let config = SessionConfig::default();
    let res = json!({
      "sid": engine.session,
      "upgrades": ["websocket"],
      "pingInterval": config.ping_interval,
      "pingTimeout": config.ping_timeout,
      "maxPayload": config.max_payload
    });

    // THIS CHANNEL IS FOR SERVER EMITTED EVENTS, we BUFFRER THEM FOR NEXT POLL...
    // IDEALLY WE SHOULD SEND THEM INTO ENGINE AS WELL ??
    let (tx, out_rx) = tokio::sync::mpsc::channel(10);
    readers.insert(engine.session, out_rx.into());
    // WE DONT KEEP REF TO EMITTER, WE JUST NEED THE RECEIVER END 
    let s = Emitter::POLL(tx.clone());
    let sio = AsyncEngine::POLL(AsyncEngineInner::new(engine, rx.into()));

    handler(sio,s);

    return Either::Right(web::Bytes::from(res.to_string()))
}

// LONG POLL POST 
async fn post_sio(query: web::Query<SessionInfo>, map:&PollChannelSenderMap, body: web::Bytes) -> HttpResponse {
    match query.sid.as_ref().and_then(|sid| map.get(sid)){
       None => HttpResponse::NotFound().finish(),
       Some(tx) => {
            tx.send(LongPollEvent::POST(body.into())).await;
            HttpResponse::Ok().finish()
       }
    }
}

// LONG POLL GET 
async fn get_sio(query: web::Query<SessionInfo>, rx:&PollChannelReceiverMap ) -> impl Responder {
    match rx.get_mut(&query.sid.unwrap()) {
       Some(rx) => {
           match rx.try_lock() {
               Ok(mut rx) => {
                   match rx.recv().await {
                       // the channel closes... so we close polls 
                       // TODO: Delete LONG POLL 
                       None => Either::Left(HttpResponse::Ok().finish()),
                       //Some(out) => Either::Right(bytes)
                       // Receive a message, send it back 
                       // TODO: BATCH / BUFFER Messages before sending back ...
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

// Actix Service for accepting LONG POLL reqs and WS ? 
pub fn socket_io<F>(path:actix_web::Resource, callback: F) -> Resource
where F: Fn(AsyncEngine, Emitter) -> () + Clone + 'static
{

    let path = {
        let callback = callback.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .guard(guard::fn_guard(|ctx| {
                ctx.head().headers().get("Upgrade").is_some_and(|v| v == "websocket")
            }))
            .to(move |req: HttpRequest, body: web::Payload| { 
                let callback = callback.clone();
                async move { create_sio_ws(req,body, callback).await }}
            )
        )
    };


    // DashMap<Sid, mpsc::Sender<LongPollEvent>>
    let poll_input: std::sync::Arc<PollChannelSenderMap> = DashMap::new().into();

    // DashMap<Sid, tokio::sync::Mutex<mpsc::Receiver<Payload>>>;
    let poll_output: std::sync::Arc<PollChannelReceiverMap> = DashMap::new().into();

    // LONG POLL GET 
    let path = { 
        let poll_output = poll_output.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .guard(guard::fn_guard(|ctx| {
                ctx.head().uri.query().is_some_and(|s| s.contains("sid"))
            }))
            .to(move |session: web::Query<SessionInfo>| { 
                let poll_output = poll_output.clone();
                async move { get_sio(session, &poll_output).await }}
            )
        )
    };

    let path = {
        let poll_output = poll_output.clone();
        let poll_input = poll_input.clone();
        let callback = callback.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .to(move |session: web::Query<SessionInfo>| { 
                let poll_input = poll_input.clone();
                let poll_output = poll_output.clone();
                let callback = callback.clone();
                async move { create_sio_poll(session, &poll_output, &poll_input, callback).await }}
            )
        )
    };

    let path = { 
        let poll_input = poll_input.clone();
        path.route(
            web::route()
            .guard(guard::Post())
            .to(move |session: web::Query<SessionInfo>, body:web::Bytes| { 
                let poll_input = poll_input.clone();
                async move { post_sio(session, &poll_input, body).await }}
            )
        )
    };
    return path
}
