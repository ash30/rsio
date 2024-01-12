use std::fmt;
use std::error;
use std::time::Duration;
use std::u8;

use crate::Participant;


pub type Sid = uuid::Uuid;


pub enum TransportState { 
    New,
    Connected,
    Closed
}

pub enum EngineInput {
    New(Option<TransportConfig>),
    Close(Participant),
    Data(Participant, Payload),
    Poll,
    Error,
    NOP
}

pub enum EngineOutput {
    Pending(Duration),
    Data(Participant, Payload),
    Closed(Option<EngineError>)

}

#[derive(Debug, Clone)]
pub enum EngineError {
    Generic,
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

impl fmt::Display for EngineOutput {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Pending(t) => write!(f, "wait {}", t.as_millis() ),
            Self::Data(Participant::Server,d) => write!(f, "data - Server emit"),
            Self::Data(Participant::Client,d) => write!(f, "data - Client Recv"),
            Self::Closed(e) => write!(f, "close")
        }
    }
}

impl fmt::Display for EngineInput {

    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let output = match self {
            Self::NOP => "NOP",
            Self::Poll => "POLL",
            Self::Error => "ERR",
            Self::New(..) => "NEW",
            Self::Data(p,d) => "DATA",
            Self::Close(..) => "CLOSE",
        };
        write!(f, "{}", output)
    }
}

use serde::{Deserialize, Serialize};


#[derive(Clone)]
pub enum Payload {
    Open(Vec<u8>),
    Close(Option<EngineError>),
    Ping,
    Pong,
    Message(Vec<u8>),
    Upgrade,
    Noop
}


impl Payload{
    pub fn as_bytes(&self, sid:Sid) -> Vec<u8>{
        let (prefix, data) = match &self{
            Payload::Open(data) => ("0", Some(data.to_owned())),
            Payload::Close(..) => ("1", None),
            Payload::Ping => ("2", None),
            Payload::Pong => ("3", None),
            Payload::Message(p) => ("4", Some(p.to_owned())),
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

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct TransportConfig { 
    pub ping_interval: u32,
    pub ping_timeout: u32,
    pub max_payload: u32
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            ping_interval:25000,
            ping_timeout: 20000,
            max_payload: 1000000,
        }
    }
}
