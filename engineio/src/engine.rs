use std::time::Duration;
use std::time::Instant;
use std::fmt;
use crate::transport::TransportError;
pub use crate::proto::*;
use std::collections::VecDeque;

pub(crate) enum TransportType {
    LongPoll,
    Websocket
}

impl fmt::Display for TransportType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::LongPoll => "long_poll",
            Self::Websocket => "websocket"
        };
        write!(f,"{}",s)
    }
}

#[trait_variant::make(AsyncTransport: Send)]
pub(crate) trait AsyncLocalTransport {
    async fn engine_state_update(&mut self, next_state:EngineState) -> Result<(),TransportError>;
    async fn recv(&mut self) -> Result<Payload,TransportError>;
    async fn send(&mut self, data:Payload) -> Result<(),TransportError>;
    fn upgrades() -> Vec<String> {
        vec![]
    }
}

pub(crate) fn default_server_state_update<T:AsyncLocalTransport>(transport:&T, sid:Sid, config:TransportConfig, next_state:EngineState) -> Option<Payload>{
    match next_state {
        EngineState::Connecting(_) => { 
            let data = serde_json::json!({
                "upgrades": <T as AsyncLocalTransport>::upgrades(),
                "maxPayload": config.max_payload,
                "pingInterval": config.ping_interval,
                "pingTimeout": config.ping_timeout,
                "sid": sid
            });
            Some(Payload::Open(serde_json::to_vec(&data).unwrap()))
        },
        EngineState::Connected(Heartbeat::Unknown(_),_) => Some(Payload::Ping),
        EngineState::Closing(t,r) => {
            match r {
                EngineCloseReason::ClientClose => Some(Payload::Noop),
                _ => Some(Payload::Close(r))
            }
        },
        _ => None
    }
}

pub(crate) fn default_client_state_update(prev_state:EngineState, next_state:EngineState) -> Option<Payload> {
    match next_state {
        EngineState::Closing(t,r) => Some(Payload::Close(EngineCloseReason::ServerClose)),
        EngineState::Closed(r) => Some(Payload::Noop),
        _ => None
    }
}

pub(crate) fn create_server_engine(sid:Sid, config:TransportConfig, now:Instant) -> impl Engine{
   BaseEngine::new(EngineState::Connecting(now), move |i:&Input,s:EngineState,now:Instant| {
        match i {
            Input::TransportUpdate(EngineState::Connecting(_),Ok(_)) => {
                Overridable::Override(Ok(Some(EngineState::Connected(Heartbeat::Alive(now), config))))
            },
            _ => Overridable::Default
        }
   })
}
pub(crate) fn create_client_engine(now:Instant) -> impl Engine {
    BaseEngine::new(EngineState::New, move |i:&Input,s:EngineState,now:Instant| {
        return match i {
            Input::Recv(Payload::Open(d)) => {
                let t = TransportConfig::default(); 
                match s {
                    EngineState::Connecting(s) => Overridable::Override(Ok(Some(EngineState::Connected(Heartbeat::Alive(now), t)))),
                    _ => Overridable::Override(Err(EngineError::Generic))
                }
            }
            _ => Overridable::Default
        }
    })
}

pub(crate) enum Input {
    Time,
    Recv(Payload),
    TransportUpdate(EngineState, Result<(),TransportError>)
}

pub(crate) enum Output {
    Data(MessageData),
    StateUpdate(EngineState),
    Wait(Instant)
}
enum Overridable<T> {
    Default,
    Override(T)
}


pub trait Engine {
    fn process_input(&mut self, i:Input, now:Instant) -> Result<(),EngineError>;
    fn poll(&mut self, now:Instant) -> Option<Output>;
    fn is_connected(&self) -> bool;
    fn close(&mut self, reason:EngineCloseReason, now:Instant);
}

pub(crate) struct BaseEngine<F> {
    state: EngineState,
    output:VecDeque<Output>,
    custom: F
}

impl<F> Engine for BaseEngine<F> 
where F: Fn(&Input,EngineState,Instant) -> Overridable<Result<Option<EngineState>,EngineError>>
{
    fn close(&mut self, reason:EngineCloseReason, now:Instant) {
        self.update_state(EngineState::Closing(now, reason))
    }


    fn is_connected(&self) -> bool {
        match self.state {
            EngineState::Connected(_,_) => true,
            _ => false
        }
    }

    fn process_input(&mut self, i:Input, now:Instant) -> Result<(),EngineError> {
        let result = if let Overridable::Override(r) = (self.custom)(&i,self.state,now) { r }
        else { self.default_processing(&i, now) }?;

        if let Input::Recv(Payload::Message(m)) = i  {
            self.output.push_back(Output::Data(m))
        }

        if let Some(next_state) = result {
            self.update_state(next_state)
        }
        Ok(())
    }

    fn poll(&mut self, now:Instant) -> Option<Output> {
        let _ = self.process_input(Input::Time, now);
        self.output.pop_front()
            .or_else(|| self.next_deadline().map(|t|Output::Wait(t)))
    }
}

impl <F> BaseEngine<F> {
    fn new(initial_state:EngineState, custom:F) -> Self {
        let mut output = VecDeque::new();
        output.push_back(Output::StateUpdate(initial_state));

        Self {
            state:initial_state,
            output,
            custom
        }
    }

    fn update_state(&mut self, s:EngineState) {
        self.state = s;
        self.output.push_back(Output::StateUpdate(s))
    }

    fn default_processing(&self, i:&Input, now:Instant) -> Result<Option<EngineState>,EngineError> {
        match i {
            Input::Time => {
                if now > self.next_deadline().unwrap_or(now) {
                    Ok(match self.state {
                        EngineState::Connecting(s) => Some(EngineState::Closing(now, EngineCloseReason::Timeout)),
                        EngineState::Connected(Heartbeat::Alive(_),c) => Some(EngineState::Connected(Heartbeat::Unknown(now),c)),
                        EngineState::Connected(Heartbeat::Unknown(_),_) => Some(EngineState::Closing(now, EngineCloseReason::Timeout)),
                        EngineState::Closing(start,r) => Some(EngineState::Closed(r)),
                        _ => None
                    })
                }
                else { Ok(None) }
            },
            Input::Recv(p) => {
                match p {
                    Payload::Close(r) => Ok(Some(EngineState::Closing(now,*r))),
                    _ => {
                        if let EngineState::Connected(_,c) = self.state {
                            Ok(Some(EngineState::Connected(Heartbeat::Alive(now),c)))
                        }
                        else { Ok(None) }
                    }
                }
            },
            Input::TransportUpdate(EngineState::Closing(_,reason),_) => Ok(Some(EngineState::Closed(*reason))),
            _ => Ok(None)
        }

    }

    fn next_deadline(&self) -> Option<Instant> {
        match self.state {
            EngineState::New => None,
            EngineState::Connecting(start) => Some(start + Duration::from_millis(5000)),
            EngineState::Connected(Heartbeat::Alive(s), config) => Some(s + Duration::from_millis(config.ping_interval)),
            EngineState::Connected(Heartbeat::Unknown(s), config) => Some(s + Duration::from_millis(config.ping_timeout)),
            EngineState::Closing(start,_) => Some(start + Duration::from_millis(5000)),
            EngineState::Closed(r) => None
        }
    }
}

#[derive(Debug,Copy,Clone)]
enum Heartbeat {
    Alive(Instant),
    Unknown(Instant)
}

impl Heartbeat {
    fn last_beat(&self) -> Instant {
        match self {
            Self::Alive(i) => *i,
            Self::Unknown(i) => *i
        }
    }
}

#[derive(Debug,Copy,Clone)]
pub(crate) enum EngineState {
    New,
    Connecting(Instant),
    Connected(Heartbeat, TransportConfig),
    Closing(Instant, EngineCloseReason),
    Closed(EngineCloseReason)
}


// =====================

#[derive(Debug, Copy, Clone)]
pub enum EngineCloseReason {
    Error(EngineError),
    Timeout,
    ServerClose,
    ClientClose
}

#[derive(Debug, Copy,Clone)]
pub enum EngineError {
    Generic,
    MissingSession,
    AlreadyClosed,
    OpenFailed,
    InvalidPoll,
    UnknownPayload,
    Transport(TransportError)
}

impl EngineError {
    pub(crate) fn is_terminal(self) -> bool {
        match self {
            _ => true
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f,"EngineError: {self:?}")
    }
}



