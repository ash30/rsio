use std::fmt;

pub type Sid = uuid::Uuid;

pub enum EngineKind {
    WS,
    LongPoll
}

pub enum TransportState { 
    Connected,
    Disconnected,
    Closed
}

#[derive(Debug)]
pub enum TransportError {
    UnknownSession,
    SessionClosed,
    MultipleInflightPollRequest,
    SessionUnresponsive,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self)
    }
}

pub enum Payload {
    Open,
    Close(Option<TransportError>),
    Ping,
    Pong,
    Message(Vec<u8>),
    Upgrade,
    Noop
}

pub struct SessionConfig { 
    pub ping_interval: u32,
    pub ping_timeout: u32,
    pub max_payload: u32
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            ping_interval:25000,
            ping_timeout: 20000,
            max_payload: 1000000,
        }
    }
}
