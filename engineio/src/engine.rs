use std::{collections::VecDeque, u8};
pub use crate::proto::*;

pub enum Participant <T> {
    Client(T),
    Server(T)
}

impl<T> Participant<T> {

    fn value(&self) -> &T {
        match self {
            Participant::Client(t) => t,
            Participant::Server(t) => t
        }
    }
}

impl <T> From<Participant<T>> for Participant<Payload> 
where T:Into<Payload> + TransportEvent
{
    fn from(value: Participant<T>) -> Self {
        match value {
            Participant::Client(v) => Participant::Client(v.into()),
            Participant::Server(v) => Participant::Server(v.into())

        }
    }
}

pub struct Engine  { 
    pub session:Sid,
    output:VecDeque<Participant<Payload>>
}

impl Engine  
{ 
    pub fn new() -> Self {
        return Self { 
            session: uuid::Uuid::new_v4(),
            output: VecDeque::new()
        }
    }

    pub fn poll_output(&mut self) -> Option<Participant<Payload>> { 
        self.output.pop_front()
    }

    pub fn consume_transport_event(&mut self, event:Participant<Payload>){ 
        // TODO: Implement state checking
        let payload = event.into();
        self.output.push_front(payload)
    }
}

// Marker Trait 
pub trait TransportEvent: Into<Payload> {}
impl TransportEvent for LongPollEvent {}
impl TransportEvent for WebsocketEvent {}

pub enum LongPollEvent {
    GET, 
    POST(Vec<u8>)
}

impl From<LongPollEvent> for Payload {
    fn from(value: LongPollEvent) -> Self {
        match value {
            LongPollEvent::POST(p) => Payload::Message(p),
            LongPollEvent::GET => Payload::Ping,
        }
    }
}

pub enum WebsocketEvent {
    Ping,
    Pong
}

impl From<WebsocketEvent> for Payload {
    fn from(value: WebsocketEvent) -> Self {
        match value {
            WebsocketEvent::Ping => Payload::Ping,
            WebsocketEvent::Pong => Payload::Pong
        }
    }
}

