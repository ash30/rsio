use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;
use actix_web::{guard, web, HttpResponse, Resource, ResponseError};
use tokio_stream::StreamExt;
use crate::io::{create_session_local, Session};
use crate::proto::{Sid, Payload, PayloadDecodeError, MessageData };
use crate::transport::{TransportKind, TransportError};
use crate::engine::{EngineError, EngineState, AsyncLocalTransport, default_server_state_update, create_server_engine, TransportType};

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

struct ActixWSAdapter {
    sid: Sid,
    config: TransportConfig,
    session:actix_ws::Session,
    msg_stream:actix_ws::MessageStream
}

impl AsyncLocalTransport for ActixWSAdapter {
    async fn send(&mut self, data:Payload) -> Result<(),crate::transport::TransportError> {
        let d = data.encode(TransportKind::Continuous);
        match data { 
            Payload::Message(MessageData::Binary(_)) => {
                self.session.binary(d).await
                    .map_err(|_|TransportError::Generic)
            }   
            _ => {
                self.session.text(String::from_utf8(d).unwrap()).await
                    .map_err(|e|TransportError::Generic)
            }
        }
    }

    async fn recv(&mut self) -> Result<Payload,crate::transport::TransportError> {
        let next = self.msg_stream.next().await;
        match next {
            None => Err(TransportError::Generic),
            Some(Err(e)) => Err(TransportError::Generic),
            Some(Ok(m)) => { 
                m.try_into().map_err(|e|TransportError::Generic)
            }
        }
    }

    async fn engine_state_update(&mut self, next_state:EngineState) -> Result<Option<Vec<u8>>,TransportError> {
        if let Some(p) = default_server_state_update(self, self.sid, self.config, next_state) {
            // TODO: Should return error to caller...
            self.send(p).await;
        }
       match next_state {
            EngineState::Closed(..) => { self.session.clone().close(None); }
            _ => {}
       };
       Ok(None)
    }
}

struct PollingTransport {
    sid:Sid,
    rx: mpsc::Receiver<PollingReqMessage>,
    config:TransportConfig,
    state: PollingState,
    buffer:Vec<Payload>,
}

enum PollingState {
    Active (mpsc::Sender<Result<Payload,EngineError>>),
    Inactive
}

impl AsyncLocalTransport for PollingTransport {
    async fn recv(&mut self) -> Result<Payload,TransportError> {
        match self.rx.recv().await {
            Some(PollingReqMessage::Poll(sender)) => {
                match &self.state {
                    PollingState::Active(s) if !s.is_closed() => {
                        let _  = sender.send(Err(EngineError::InvalidPoll)).await;
                        Err(TransportError::Generic)
                    }
                    _ => {
                        // TODO: Better error  please
                        for x in self.buffer.drain(..) {
                            let _ = sender.send(Ok(x)).await;
                        }
                        self.state = PollingState::Active(sender);
                        Ok(Payload::Pong)
                    }
                }
            }
            Some(PollingReqMessage::Data(p)) => {
                Ok(p)
            },
            None => { 
                // Router has droped sender, consider transport to be closed
                Err(TransportError::Generic)
            }
        }
    }

    async fn send(&mut self, data:Payload) -> Result<(),TransportError> {
        match &self.state {
            PollingState::Active(sender) => {
                if let Err(e) = sender.send(Ok(data)).await {
                    self.state = PollingState::Inactive;
                    self.buffer.push(e.0.unwrap());
                    Ok(())
                }
                else { 
                    Ok(())
                }
            }
            PollingState::Inactive => {
                self.buffer.push(data);
                Ok(())
            }
        }
    }

    async fn engine_state_update(&mut self, next_state:EngineState) -> Result<Option<Vec<u8>>,TransportError> {
        if let Some(p) = default_server_state_update(self, self.sid, self.config, next_state) {
            let _ = self.send(p).await;
        }
        match next_state {
            EngineState::Closed(e) => {
                self.rx.close();
            }
            _ => {}
        };
        Ok(None)
    }

    fn upgrades() -> Vec<String> {
        vec![TransportType::Websocket.to_string()]
    }
}



struct PollingTransportRouter {
    txs: Mutex<HashMap<Sid, mpsc::Sender<PollingReqMessage>>>
}

enum PollingReqMessage {
    Poll(mpsc::Sender<Result<Payload,EngineError>>),
    Data(Payload)
}

impl PollingTransportRouter {
    fn create(&self, sid:Sid, config:TransportConfig) -> PollingTransport {
        let (tx,rx) = mpsc::channel(32);
        self.txs.lock().unwrap().insert(sid.clone(), tx);

        PollingTransport {
            sid,
            buffer: Vec::new(),
            config,
            rx,
            state: PollingState::Inactive
        }
    }

    fn delete(&self, sid:Sid) -> Result<(),EngineError> {
       self.txs.lock().unwrap().remove(&sid).ok_or(EngineError::MissingSession).map(|_|())
    }

    async fn post(&self, sid:Sid, data:&[u8]) -> Result<(),EngineError> {
        let tx = self.txs.lock()
            .unwrap()
            .get(&sid)
            .map(|t| t.clone())
            .ok_or(EngineError::MissingSession)?;

        let v = Payload::decode_combined(data, TransportKind::Poll);
        match v {
            Err(_) => {
                drop(tx);
                self.txs.lock().unwrap().remove(&sid);
                return Err(EngineError::UnknownPayload)
            },
            Ok(v) => {
                for p in v {
                    tx.send(PollingReqMessage::Data(p)).await;
                }
            }
        };
        Ok(())
    }

    async fn poll(&self, sid:Sid) -> Result<Vec<u8>,EngineError> {
        let tx = self.txs.lock()
            .unwrap()
            .get(&sid)
            .map(|t| t.clone())
            .ok_or(EngineError::MissingSession)?;


        if tx.is_closed() {
            let _ = self.delete(sid);
            return Err(EngineError::MissingSession);
        }

        let (res_tx, mut res_rx) = mpsc::channel(32);
        tx.send(PollingReqMessage::Poll(res_tx)).await;
        
        match res_rx.recv().await {
            Some(Ok(first)) => {
                let mut buffer = vec![first];
                let deadline = Instant::now() + Duration::from_millis(10);
                loop {
                    match tokio::time::timeout_at(deadline,res_rx.recv()).await {
                        Err(_) => break, // Timeout 
                        Ok(None) => break, // Transport Close, just return existing buffer...
                        Ok(Some(Err(_))) => break, // Poll Error ? shouldnt receive this....
                        Ok(Some(Ok(p))) => {
                            buffer.push(p)
                        }
                    };
                };
                res_rx.close();
                let data = Payload::encode_combined(&buffer, TransportKind::Poll);
                // TODO: DRYer please
                if tx.is_closed() {
                    let _ = self.delete(sid);
                }
                Ok(data)
            },
            Some(Err(e)) => Err(e),
            None => {
                Err(EngineError::Generic) // Transport Closed?
            }
        }
    }
}

use tokio::sync::mpsc;
pub fn engine_io(path:actix_web::Resource, config:TransportConfig, service:fn(IOEngine)) -> Resource {
    let poll_router = Arc::new(PollingTransportRouter { txs: HashMap::new().into() });

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
                    let mut engine = create_server_engine(sid, config, Instant::now().into());
                    let session = create_session_local(engine, ActixWSAdapter { sid, config, session, msg_stream });
                    service(session);
                    return Ok::<HttpResponse, EngineError>(response);
                }
            })
        )
    };

    // LONG POLL GET 
    let path = { 
        let poll_router = poll_router.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .guard(guard::fn_guard(|ctx| {
                ctx.head().uri.query().is_some_and(|s| s.contains("sid"))
            }))
            .to(move |session: web::Query<SessionInfo>| { 
                let poll_router = poll_router.clone();
                async move {
                    let Some(sid) = session.sid else {
                        return EngineError::MissingSession.error_response()
                    };
                    match poll_router.poll(sid).await {
                        Ok(vec) => HttpResponse::Ok().body(vec),
                        Err(e) => e.error_response()
                    }
                }
            })
        )
    };

    // LONG POLL CREATE
    let path = {
        let poll_router = poll_router.clone();
        path.route(
            web::route()
            .guard(guard::Get())
            .to(move |_session: web::Query<SessionInfo>| {
                let poll_router = poll_router.clone();
                let sid = uuid::Uuid::new_v4();
                let engine = create_server_engine(sid, config, Instant::now().into());
                let transport = poll_router.create(sid, config);
                let session = create_session_local(engine, transport);
                service(session);
                async move {
                    match poll_router.poll(sid).await {
                        Ok(vec) => HttpResponse::Ok().body(vec),
                        Err(e) => e.error_response()
                    }
                }
            })
        )
    };

    // POST MSG LONG POLL
    let path = { 
        let poll_router = poll_router.clone();
        path.route(
            web::route()
            .guard(guard::Post())
            .to(move |session: web::Query<SessionInfo>, body:web::Bytes| { 
                let poll_router = poll_router.clone();
                async move { 
                    let poll_router = poll_router.clone();
                    let Some(sid) = session.sid else {
                        return EngineError::MissingSession.error_response()
                    };
                    match poll_router.post(sid, body.as_ref()).await {
                        Ok(_) => HttpResponse::Ok().body("ok"),
                        Err(e) => e.error_response()
                    }
                }
            }
            )
        )
    };

    let path = {
        let poll_router = poll_router.clone();
        path.route(
            web::route()
            .guard(guard::Delete())
            .to(move | session:web::Query<SessionInfo>| {
                let poll_router = poll_router.clone();
                async move { 
                    let Some(sid) = session.sid else {
                        return EngineError::MissingSession.error_response()
                    };
                    let res = poll_router.delete(sid);
                    match res {
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


