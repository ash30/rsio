use std::fmt;
use std::error;
use std::time::Duration;
use std::u8;

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
    NOP,
    Close(Participant),
    Error,
    Poll,
    Data(Participant, Payload),
}

pub enum EngineOutput {
    Pending(Duration),
    Data(Participant, Payload),
    Closed(Option<EngineError>)

}

#[derive(Debug, Clone)]
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

#[derive(Clone)]
pub enum Payload {
    Open,
    Close(Option<EngineError>),
    Ping,
    Pong,
    Message(Vec<u8>),
    Upgrade,
    Noop
}


impl Payload{
    pub fn as_bytes(&self) -> Vec<u8>{
        let (prefix, data) = match &self{
            Payload::Open => ("0", None),
            Payload::Close(..) => ("1", None),
            Payload::Ping => ("2", None),
            Payload::Pong => ("3", None),
            Payload::Message(p) => ("4", Some(p)),
            Payload::Upgrade => ("5",None),
            Payload::Noop => ("6",None),
        };

        let mut b = prefix.as_bytes().to_owned();
        if let Some(data) = data {
            b = [b,data.clone()].concat();
        }
        return b
    }
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
