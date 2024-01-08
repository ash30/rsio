use std::fmt;
use std::error;

use crate::Participant;


pub type Sid = uuid::Uuid;

pub enum EngineKind {
    Continuous,
    Poll
}

pub enum TransportState { 
    Connected,
    Closed
}

pub enum EngineInput {
    Error,
    PollStart,
    PollEnd,
    Data(Participant, Payload),
}

pub enum EngineOutput {
    Pending,
    Data(Participant, Payload),
    Closed(Option<EngineError>)

}

#[derive(Debug)]
pub enum EngineError {
    UnknownSession,
    SessionAlreadyClosed,
    InvalidPollRequest,
    SessionUnresponsive,
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self)
    }
}

pub enum Payload {
    Open,
    Close(Option<EngineError>),
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
