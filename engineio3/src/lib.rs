use std::{collections::VecDeque, ops::Add, time::{self, Duration, Instant}};
use serde::{Serialize,Deserialize};
use serde_repr::*;

#[derive(thiserror::Error, Debug)]
pub enum EngineError {
    #[error("Bad Ingress Message")]
    MessageDecodeError(#[source] serde_json::Error),

    #[error("Unknown Error")]
    Unknown
}

// ===============================
#[derive(Debug)]
pub enum Message<T>{
    Text(T),
    Binary(T)
}
impl<T> Message<T> {
    pub fn value(self) -> T {
        match self {
            Message::Text(t) => t,
            Message::Binary(t) => t
        }
    }
}

pub trait RawPayload {
    type U;
    fn prefix(&self) -> u8;
    fn body(&self) -> Option<Self::U>;
    fn body_as_bytes(&self) -> Vec<u8>;
}

#[derive(Debug)]
pub enum Payload<T> {
    Open(OpenMessage),
    Close(CloseReason),
    Ping,
    Pong,
    Msg(Message<T>),
    Upgrade,
    Noop
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct OpenMessage {
    ping_interval: u64,
    ping_timeout: u64
}

#[derive(Clone, Copy, Debug)]
pub struct SessionConfig {
    ping_interval: time::Duration,
    ping_timeout: time::Duration,
}

impl From<OpenMessage> for SessionConfig {
    fn from(value: OpenMessage) -> Self {
        Self {
            ping_timeout: Duration::from_millis(value.ping_timeout),
            ping_interval: Duration::from_millis(value.ping_interval)
        }
    }
}

#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug, Copy, Clone)]
#[repr(u8)]
pub enum CloseReason {
    Unknown,
    TransportTimeout,
    TransportError,
    ServerClose,
    ClientClose,
}
//0{"ping_interval":15000, "ping_timeout":15000}
// ===============================

impl <T> From<Message<T>> for Payload<T> {
    fn from(value: Message<T>) -> Self {
        Payload::Msg(value)
    }
}
pub fn decode<U>(data:impl RawPayload<U = U>) -> Result<Payload<U>,EngineError>{
        match data.prefix() {
            b'0' => {
                let v = data.body_as_bytes();
                let e = serde_json::from_slice(v.as_slice()).map_err(EngineError::MessageDecodeError)?;
                Ok(Payload::Open(e))
            },
            b'1' => {
                let v = data.body_as_bytes();
                let e = serde_json::from_slice(v.as_slice()).map_err(EngineError::MessageDecodeError)?;
                Ok(Payload::Close(e))
            },
            b'2' => Ok(Payload::Ping),
            b'3' => Ok(Payload::Pong),
            b'4' => {
                let body = data.body().ok_or(EngineError::Unknown)?;
                Ok(Payload::Msg(Message::Text(body)))
            },
            b'5' => Ok(Payload::Upgrade),
            b'6' => Ok(Payload::Noop),
            _ => Err(EngineError::Unknown)
        }
}


impl <T> Payload<T> {
    fn prefix(&self) -> u8 {
        match self {
            Payload::Open(_) => b'0',
            Payload::Close(_) => b'1',
            Payload::Ping => b'2',
            Payload::Pong => b'3',
            Payload::Msg(_) => b'4',
            Payload::Upgrade => b'5',
            Payload::Noop => b'6'
        }
    }

    pub fn into_bytes(&self) -> Vec<u8> {
        let (a,b) = match self {
            Payload::Open(data) => (b'0', Some(serde_json::to_vec(data))),
            Payload::Close(data) => (b'1', Some(serde_json::to_vec(data))),
            Payload::Ping => (b'2', None),
            Payload::Pong => (b'3', None),
            Payload::Msg(Message::Text(_)) => (b'4', None),
            Payload::Msg(Message::Binary(_)) => (b'b', None),
            Payload::Upgrade => (b'5', None),
            Payload::Noop => (b'6', None)
        };
        let mut v = vec![];
        v.push(a);
        if let Some(Ok(mut d)) = b {
            v.append(&mut d);
        };
        v
    }
}



#[derive(Debug)]
pub struct Engine<D,T> {
    buffer: VecDeque<Payload<D>>,
    state: EngineState<T>,
}

impl<D,T> Engine<D,T> where T:PartialOrd+PartialEq+Add<Duration,Output = T>+Copy+Clone{
    pub fn new(now:T) -> Self {
        Self {
            buffer:VecDeque::new(),
            state: EngineState::New(now),
        }
    }

    // We pass paylaod into engine 
    // to update state 
    // but engine doesn't own payload 
    pub fn handle_input(&mut self, p:&Payload<D>, now:T) -> Result<(),EngineError>{
        self.poll_next_state(now);
        let n = match self.state {
            EngineState::New(_) => {
                match p {
                    Payload::Open(config) => {
                        EngineState::Opened(Heartbeat::new(now), (*config).into())
                    }
                    Payload::Close(r) => EngineState::Closed(*r),
                    _ => return Err(EngineError::Unknown)
                }
            }
            EngineState::Opened(_,c) => {
                match p {
                    Payload::Open(_) => return Err(EngineError::Unknown),
                    Payload::Close(r) => EngineState::Closed(*r),
                    Payload::Ping => {
                        self.buffer.push_back(Payload::Pong);
                        return Ok(())
                    }
                    _ => EngineState::Opened(Heartbeat::new(now), c)
                }
            },
            EngineState::Closed(_) => return Err(EngineError::Unknown)
        };
        self.state = n;
        Ok(())
    }

    pub fn poll_output(&mut self, now:T) -> Option<Payload<D>> {
        self.poll_next_state(now);
        self.buffer.pop_back()
    }
    
    fn poll_next_state(&mut self, now:T) {
        let Some(n) = self.next_deadline() else { return };
        if now < n { return };
        let new_state = match self.state {
            EngineState::New(_) => EngineState::Closed(CloseReason::TransportTimeout),
            EngineState::Opened(Heartbeat::Alive(i),c) => EngineState::Opened(Heartbeat::Unknown(i), c),
            EngineState::Opened(Heartbeat::Unknown(_),_) => EngineState::Closed(CloseReason::TransportTimeout),
            EngineState::Closed(_) => return 
        };

        self.state = new_state;
    }

    pub fn next_deadline(&self) -> Option<T> {
        let d = match self.state { 
            EngineState::New(s) => s + Duration::from_millis(5000),
            EngineState::Opened(Heartbeat::Alive(i),c) => i + c.ping_interval,
            EngineState::Opened(Heartbeat::Unknown(i),c) => i + c.ping_interval + c.ping_timeout,
            EngineState::Closed(_) => return None
        };
        Some(d)
    }
}

#[derive(Clone, Copy, Debug)]
enum EngineState<TIME> {
    New(TIME),
    Opened(Heartbeat<TIME>, SessionConfig),
    Closed(CloseReason)
}

#[derive(Copy, Clone, PartialEq,Debug)]
pub enum Heartbeat<TIME> {
    Alive(TIME),
    Unknown(TIME),
}

impl<T> Heartbeat<T> {
    fn new(now:T) -> Self {
        Heartbeat::Alive(now)
    }
}
