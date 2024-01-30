use std::u8;
use std::vec;
use crate::EngineCloseReason;
use crate::EngineKind;
pub type Sid = uuid::Uuid;
use serde::{Deserialize, Serialize};

#[derive(Clone,Debug)]
pub enum MessageData {
    String(Vec<u8>),
    Binary(Vec<u8>)
}

impl MessageData {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            MessageData::String(v) => v,
            MessageData::Binary(v) => v,
        }
    }
}

#[derive(Clone,Debug)]
pub enum Payload {
    Open(Vec<u8>),
    Close(EngineCloseReason),
    Ping,
    Pong,
    Message(MessageData),
    Upgrade,
    Noop
}

#[derive(Debug)]
pub enum PayloadDecodeError {
    InvalidFormat,
    UnknownType
}

static EMPTY_DATA: [u8;0] = [];

impl Payload{
    fn as_bytes(&self) -> &[u8] {
        match self { 
            Payload::Open(data) =>data.as_slice(),
            Payload::Close(reason) => EMPTY_DATA.as_slice(),
            Payload::Ping => EMPTY_DATA.as_slice(),
            Payload::Pong => EMPTY_DATA.as_slice(),
            Payload::Message(p) => p.as_bytes(),
            Payload::Upgrade => EMPTY_DATA.as_slice(),
            Payload::Noop => EMPTY_DATA.as_slice()
        }
    }

    pub fn encode(&self, transport: EngineKind) -> Vec<u8> {
        let header:Option<u8> = match self {
            Payload::Open(..) => b'0'.into(),
            Payload::Close(..) => b'1'.into(),
            Payload::Ping => b'2'.into(),
            Payload::Pong => b'3'.into(),
            Payload::Message(MessageData::String(..)) => b'4'.into(), 
            Payload::Message(MessageData::Binary(..)) => { 
                match transport {
                    EngineKind::Poll => b'b'.into(),
                    EngineKind::Continuous => None,
                }
            }, 
            Payload::Upgrade => b'5'.into(),
            Payload::Noop => b'6'.into()
        };
        let d = self.as_bytes();
        let mut v = Vec::with_capacity(d.len()+10);
        if let Some(c) = header { v.push(c) };
        v.extend_from_slice(d);
        return v
    }

    pub fn decode(data:&[u8], transport: EngineKind) -> Result<Payload, PayloadDecodeError> {
        let t = data.first();
        match t {
            None => Err(PayloadDecodeError::InvalidFormat),
            Some(n) => {
                let data = data.get(1..).and_then(|a| Some(a.to_vec())).unwrap_or(vec![]);
                match n {
                    b'0' => Ok(Payload::Open(data)),
                    b'1' => Ok(Payload::Close(EngineCloseReason::Timeout)),
                    b'2' => Ok(Payload::Ping),
                    b'3' => Ok(Payload::Pong),
                    b'4' => Ok(Payload::Message(MessageData::String(data))),
                    b'b' => Ok(Payload::Message(MessageData::Binary(data))),
                    b'5' => Ok(Payload::Upgrade),
                    b'6' => Ok(Payload::Noop),
                    _ => Err(PayloadDecodeError::UnknownType)
                }
            }
        }

    }

    pub fn decode_combined(v:&[u8], t: EngineKind) -> Vec<Result<Payload,PayloadDecodeError>> {
        v.split(|c| *c == b'\x1e').into_iter()
            .map(|d| Payload::decode(d, t))
            .collect()
    }

    pub fn encode_combined(v:&[Payload], t:EngineKind) -> Vec<u8> {
        let seperator:u8 = b'\x1e';
        let start = v.first().map(|p| p.encode(t));
        let rest = v.get(1..).map(|s| s.iter().flat_map(|p| vec![vec![seperator], p.encode(t).to_vec()].concat()).collect::<Vec<u8>>());
        return start.and_then(|mut n| rest.map(|r| {n.extend_from_slice(&r); n})).unwrap_or(vec![])
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

