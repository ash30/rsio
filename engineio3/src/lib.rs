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
    Open(OpenData),
    Close(CloseData),
    Ping,
    Pong,
    Msg(Message<T>),
    Upgrade,
    Noop
}

#[derive(Serialize, Deserialize)]
pub struct OpenData {
    interval: time::Duration, timeout: time::Duration 
}

#[derive(Serialize, Deserialize)]
pub struct CloseData(CloseReason);

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


#[derive(Serialize_repr, Deserialize_repr, PartialEq, Debug)]
#[repr(u8)]
pub enum CloseReason {
    Unknown,
    TransportClose,
    ServerClose,
    ClientClose,
}


pub struct Engine<T> {
    buffer: VecDeque<Payload<T>>
}

impl<T> Default for Engine<T> {
    fn default() -> Self {
        Self {
            buffer:VecDeque::new()
        }
    }
}

impl<T> Engine<T> {


    // We pass paylaod into engine 
    // to update state 
    // but engine doesn't own payload 
    pub fn handle_input(&mut self, p:&Payload<T>) -> Result<(),Error>{
        Ok(())
    }

    pub fn poll(&mut self, now:Instant) -> Option<Payload<T>> {
        None
    }

    pub fn next_timeout(&self) -> Option<Duration> {
        None
    }


}

/*

#[derive(Clone, Copy)]
pub enum EngineState {
    New,
    Opened(Heartbeat, Config),
    Closed
}

#[derive(Clone, Copy)]
pub struct Config {
    ping_interval: time::Duration,
    ping_timeout: time::Duration,
}

#[derive(Clone, Copy)]
struct Heartbeat {}

fn ClientEngine<U>(config:Config) -> Engine<U> {
    let reducer = |e,s| {
        return Ok(None)
    };
    return Engine::<U> {
        buf: VecDeque::new(),
        state: EngineState::New,
        reducer
    }
}

pub struct Engine<U> {
    buf: VecDeque<Payload<U>>,
    state: EngineState,
    reducer: fn(Event, EngineState) -> Result<Option<EngineState>,Error>
}

impl <U> Engine<U> {
    pub fn handle<'a,T,S>(&mut self, input:S) -> Result<Option<Payload<T>>,Error>
        where 
        T:ToOwned<Owned=U>, 
        T:Into<&'a[u8]>, 
        S:TryInto<Payload<T>>, 
    {
        todo!()
        //let i = input.try_into().map_err(|_|Error {  })?;
        //let e = match i {
        //    PayloadType::Msg(b) => {
        //        return Ok(Some(PayloadType::Msg(b)))
        //    }
        //    PayloadType::Open(d) =>  serde_json::from_slice(d.into()).map_err(|_|Error{})?,
        //    _ => Event::Touch
        //};
        //let next = (self.reducer)(e, self.state)?;
        //if let Some(n) = next {
        //    self.state = n
        //}
        //Ok(None)
    }

    // Handle will sync return any data owning payloads 
    // generally we will only store light payloads for dispatch
    pub fn poll(&mut self, now:time::Instant) -> (Option<Payload<U>>, Option<time::Instant>) {
        todo!()
    }

    pub fn is_closed(&self) -> bool {
        match self.state {
            EngineState::Closed => true,
            _ => false
        }
    }
}

*/
