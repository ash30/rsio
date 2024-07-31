use std::{collections::VecDeque, time::{self, Duration, Instant}};
use serde::{Serialize,Deserialize};
use serde_repr::*;

pub struct Error {}

// ===============================
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

pub enum Payload<T> {
    Open(SessionConfig),
    Close(CloseReason),
    Ping,
    Pong,
    Msg(Message<T>),
    Upgrade,
    Noop
}


#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct SessionConfig {
    ping_interval: time::Duration,
    ping_timeout: time::Duration,
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

// ===============================

impl <T> From<Message<T>> for Payload<T> {
    fn from(value: Message<T>) -> Self {
        Payload::Msg(value)
    }
}
pub fn decode<U>(data:impl RawPayload<U = U>) -> Result<Payload<U>,Error>{
        match data.prefix() {
            b'0' => {
                let v = data.body_as_bytes();
                let e = serde_json::from_slice(v.as_slice()).map_err(|_|Error{})?;
                Ok(Payload::Open(e))
            },
            b'1' => {
                let v = data.body_as_bytes();
                let e = serde_json::from_slice(v.as_slice()).map_err(|_|Error{})?;
                Ok(Payload::Close(e))
            },
            b'2' => Ok(Payload::Ping),
            b'3' => Ok(Payload::Pong),
            b'4' => {
                let body = data.body().ok_or(Error {})?;
                Ok(Payload::Msg(Message::Text(body)))
            },
            b'5' => Ok(Payload::Upgrade),
            b'6' => Ok(Payload::Noop),
            _ => Err(Error {})
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



pub struct Engine<T> {
    buffer: VecDeque<Payload<T>>,
    state: EngineState,
}

impl<T> Engine<T> {
    pub fn new(now:Instant) -> Self {
        Self {
            buffer:VecDeque::new(),
            state: EngineState::New(now),
        }
    }

    // We pass paylaod into engine 
    // to update state 
    // but engine doesn't own payload 
    pub fn handle_input(&mut self, p:&Payload<T>, now:Instant) -> Result<(),Error>{
        self.poll_next_state(now);
        let n = match self.state {
            EngineState::New(_) => {
                match p {
                    Payload::Open(config) => {
                        EngineState::Opened(Heartbeat::new(now), *config)
                    }
                    Payload::Close(r) => EngineState::Closed(*r),
                    _ => return Err(Error{})
                }
            }
            EngineState::Opened(_,c) => {
                match p {
                    Payload::Open(_) => return Err(Error{}),
                    Payload::Close(r) => EngineState::Closed(*r),
                    Payload::Ping => {
                        self.buffer.push_back(Payload::Pong);
                        return Ok(())
                    }
                    _ => EngineState::Opened(Heartbeat::new(now), c)
                }
            },
            EngineState::Closed(_) => return Err(Error{})
        };
        self.state = n;
        Ok(())
    }

    pub fn poll_output(&mut self, now:Instant) -> Option<Payload<T>> {
        self.poll_next_state(now);
        self.buffer.pop_back()
    }
    
    fn poll_next_state(&mut self, now:Instant) {
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

    pub fn next_deadline(&self) -> Option<Instant> {
        let d = match self.state { 
            EngineState::New(s) => s + Duration::from_millis(5000),
            EngineState::Opened(Heartbeat::Alive(i),c) => i + c.ping_interval,
            EngineState::Opened(Heartbeat::Unknown(i),c) => i + c.ping_interval + c.ping_timeout,
            EngineState::Closed(_) => return None
        };
        Some(d)
    }
}

#[derive(Clone, Copy)]
enum EngineState {
    New(Instant),
    Opened(Heartbeat, SessionConfig),
    Closed(CloseReason)
}

#[derive(Copy, Clone, PartialEq)]
pub enum Heartbeat {
    Alive(Instant),
    Unknown(Instant),
}

impl Heartbeat {
    fn new(now:Instant) -> Self {
        Heartbeat::Alive(now)
    }
}
