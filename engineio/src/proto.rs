use std::fmt;
use std::time::Duration;
use std::u8;

use crate::Participant;
use crate::EngineCloseReason;
pub type Sid = uuid::Uuid;

#[derive(Debug)]
pub enum EngineInput {
    New(Option<TransportConfig>, EngineKind),
    Close(Participant),
    Data(Participant, Result<Payload,PayloadDecodeError>),
    Poll,
    Listen,
    Tock
}

#[derive(Debug)]
pub enum EngineKind {
    Poll,
    Continuous
}


#[derive(Debug)]
pub enum EngineOutput {
    Tick { length:Duration },
    SetIO(Participant, bool),
    Data(Participant, Payload),
}

#[derive(Debug, Clone)]
pub enum EngineError {
    Generic,
    MissingSession,
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


impl fmt::Display for EngineInput {

    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let output = match self {
            Self::Tock  => "Tock",
            Self::Poll => "POLL",
            Self::Listen => "LISTEN",
            Self::New(..) => "NEW",
            Self::Data(p,d) => "DATA",
            Self::Close(..) => "CLOSE",
        };
        write!(f, "{}", output)
    }
}

use serde::{Deserialize, Serialize};


#[derive(Clone,Debug)]
pub enum Payload {
    Open(Vec<u8>),
    Close(EngineCloseReason),
    Ping,
    Pong,
    Message(Vec<u8>),
    Upgrade,
    Noop
}

#[derive(Debug)]
pub enum PayloadDecodeError {
    InvalidFormat,
    UnknownType
}

impl TryFrom<&[u8]> for Payload {
    type Error = PayloadDecodeError;
    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let t = value.first();
        match t {
            None => Err(PayloadDecodeError::InvalidFormat),
            Some(n) => {
                let data = value.get(1..).and_then(|a| Some(a.to_vec())).unwrap_or(vec![]);
                match n {
                    b'0' => Ok(Payload::Open(data)),
                    b'1' => Ok(Payload::Close(EngineCloseReason::Timeout)),
                    b'2' => Ok(Payload::Ping),
                    b'3' => Ok(Payload::Pong),
                    b'4' => Ok(Payload::Message(data)),
                    b'5' => Ok(Payload::Upgrade),
                    b'6' => Ok(Payload::Noop),
                    _ => Err(PayloadDecodeError::UnknownType)
                }
            }
        }
    }
}

impl Payload{
    pub fn as_bytes(&self) -> Vec<u8>{
        let (prefix, data) = match &self{
            Payload::Open(data) => ("0", Some(data.to_owned())),
            Payload::Close(reason) => ("1", None),
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

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
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

fn is_binary(d:&[u8]) -> bool {
    d.first().filter(|c| **c == b'b').is_some()
}
