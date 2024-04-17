use std::time::Duration;
use std::time::Instant;
use std::fmt;
use crate::transport::TransportError;
pub use crate::proto::*;
use std::collections::VecDeque;

pub(crate) trait AsyncTransport {
    async fn engine_state_update(&mut self, next_state:EngineState);
    async fn recv(&mut self) -> Result<Payload,TransportError>;
    async fn send(&mut self, data:Payload) -> Result<(),TransportError>;
}

pub(crate) fn default_server_state_update(next_state:EngineState) -> Option<Payload>{
    match next_state {
        //EngineState::Connected(h) => Some(Payload::Open()),
        EngineState::Closing(t,r) => Some(Payload::Close(EngineCloseReason::ServerClose)),
        EngineState::Closed(r) => Some(Payload::Noop),
        _ => None
    }
}

pub(crate) fn default_client_state_update(next_state:EngineState) -> Option<Payload> {
    match next_state {
        EngineState::Closing(t,r) => Some(Payload::Close(EngineCloseReason::ServerClose)),
        EngineState::Closed(r) => Some(Payload::Noop),
        _ => None
    }
}

fn default_transport_observation(p:&Payload) -> Option<EngineState> {
    match p {
        Payload::Close(r) => Some(EngineState::Closed(*r)),
        _ => None
    }
}

struct TransportObserver(fn(&Payload) -> Option<EngineState>);

pub(crate) fn create_server_engine(config:TransportConfig, now:Instant) -> Engine {
    Engine::new(EngineState::Connected(Heartbeat::Alive(now), config), TransportObserver(|p|{
        match p {
            _ => default_transport_observation(p)
        }
    }))
}


pub(crate) struct Engine {
    state: EngineState,
    callback:TransportObserver,
    output:VecDeque<Output>
}

pub(crate) enum Output {
    Send(Payload),
    Recv(MessageData),
    State(EngineState),
    Wait(Instant)
}

impl Engine {
    pub fn new(initial_state:EngineState, callback:TransportObserver) -> Self {
        Self {
            state:initial_state,
            callback,
            output: VecDeque::new()
        }
    }

    pub fn poll(&mut self, now:Instant) -> Option<Output> {
        self.update_time(now);    
        self.output.pop_front()
            .or_else(|| self.next_deadline().map(|t|Output::Wait(t)))
    }

    pub fn send(&mut self, m:MessageData, now:Instant) -> Result<(),EngineError> {
        self.update_time(now);
        match self.state {
            EngineState::Closing(..) => Err(EngineError::AlreadyClosed),
            EngineState::Closed(..) => Err(EngineError::AlreadyClosed),
            _ => Ok(())
        }?;
        self.output.push_back(Output::Send(Payload::Message(m)));
        Ok(())
    }

    pub fn recv(&mut self, p:Payload, now:Instant) -> Result<(),EngineError> { 
        self.update_time(now);
        match self.state {
            EngineState::Closed(_) => Err(EngineError::AlreadyClosed),
            _ => Ok(())
        }?;

        let next = self.callback.0(&p);
        match p {
            Payload::Message(m) => { 
                self.output.push_back(Output::Recv(m));
            },
            _ => {}
        }
        if let Some(n) = next {
            self.update_state(n)
        }
        Ok(())
    }

    fn update_time(&mut self, now:Instant) {
        let deadline = self.next_deadline();
        if now > deadline.unwrap_or(now) {
            let next = match self.state {
                EngineState::New(s) => Some(EngineState::Closing(now, EngineCloseReason::Timeout)),
                EngineState::Connected(Heartbeat::Alive(_),c) => Some(EngineState::Connected(Heartbeat::Unknown(now), c)),
                EngineState::Connected(Heartbeat::Unknown(_),c) => Some(EngineState::Closing(now, EngineCloseReason::Timeout)),
                EngineState::Closing(start,r) => Some(EngineState::Closing(now, r)),
                EngineState::Closed(r) => None
            };
            if let Some(n) = next { self.update_state(n) }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        match self.state {
            EngineState::New(start) => Some(start + Duration::from_millis(5000)),
            EngineState::Connected(Heartbeat::Alive(s), config) => Some(s + Duration::from_millis(config.ping_interval)),
            EngineState::Connected(Heartbeat::Unknown(s), config) => Some(s + Duration::from_millis(config.ping_timeout)),
            EngineState::Closing(start,_) => Some(start + Duration::from_millis(5000)),
            EngineState::Closed(r) => None
        }
    }

    fn update_state(&mut self, s:EngineState) {
        self.state = s;
        self.output.push_back(Output::State(s))
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
    New(Instant),
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



