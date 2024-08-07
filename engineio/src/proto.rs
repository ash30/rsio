use std::u8;
use std::vec;
pub type Sid = uuid::Uuid;

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
    Close(Vec<u8>),
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

    pub fn encode(&self) -> Vec<u8> {
        let header:Option<u8> = match self {
            Payload::Open(..) => b'0'.into(),
            Payload::Close(..) => b'1'.into(),
            Payload::Ping => b'2'.into(),
            Payload::Pong => b'3'.into(),
            Payload::Message(MessageData::String(..)) => b'4'.into(), 
            Payload::Message(MessageData::Binary(..)) => { 
                None
                //match transport {
                //    TransportKind::Poll => b'b'.into(),
                //    TransportKind::Continuous => None,
                //}
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

    pub fn decode(data:&[u8]) -> Result<Payload, PayloadDecodeError> {
        let t = data.first();
        match t {
            None => Err(PayloadDecodeError::InvalidFormat),
            Some(n) => {
                let data = data.get(1..).and_then(|a| Some(a.to_vec())).unwrap_or(vec![]);
                match n {
                    b'0' => Ok(Payload::Open(data)),
                    b'1' => Ok(Payload::Close(data)),
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

    pub fn decode_combined(v:&[u8]) ->Result<Vec<Payload>,PayloadDecodeError> {
        v.split(|c| *c == b'\x1e').into_iter()
            .map(|d| Payload::decode(d))
            .collect()
    }

    pub fn encode_combined(v:&[Payload]) -> Vec<u8> {
        let seperator:u8 = b'\x1e';
        let start = v.first().map(|p| p.encode());
        let rest = v.get(1..).map(|s| s.iter().flat_map(|p| vec![vec![seperator], p.encode().to_vec()].concat()).collect::<Vec<u8>>());
        return start.and_then(|mut n| rest.map(|r| {n.extend_from_slice(&r); n})).unwrap_or(vec![])
    }
}


