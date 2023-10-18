use std::{collections::VecDeque, u8};
pub use crate::proto::*;

pub type Sid = uuid::Uuid;
pub enum Error {}

pub enum Output {
    Send(Payload),
    Receive(Payload),
    TransportChange(TransportState),
    Pending
}

pub struct Engine<T>  { 
    pub session:Sid,
    transport:T,
    output:VecDeque<Output>
}

impl Engine<LongPoll> {
    pub fn new_longpoll() -> Self {
        return Self { 
            session:uuid::Uuid::new_v4(), 
            transport:LongPoll {},
            output:VecDeque::new() 
        } 
    }
}
impl Engine<Websocket> {
    pub fn new_ws() -> Self {
        return Self { 
            session:uuid::Uuid::new_v4(),
            transport:Websocket {},
            output:VecDeque::new()  
        } 
    }
}

impl <T:Transport> Engine<T>  
{ 
    pub fn poll_output(&mut self) -> Output { 
        self.output.pop_front().unwrap_or(Output::Pending)
    }

    pub fn consume_transport_event(&mut self, e:T::Event){ 
        self.transport.consume(e, &mut self.output)
    }
}


pub trait Transport {
    type Event;

    fn upgrades() -> Option<EngineKind> { 
       None
    }

    fn consume(&mut self, event:Self::Event, buf: &mut VecDeque<Output>) {
    }
}

pub struct LongPoll {} 

pub enum LongPollEvent {
    GET, 
    POST(Vec<u8>)
}

impl Transport for LongPoll { 
    type Event = LongPollEvent;

    fn upgrades() -> Option<EngineKind> {
        Some(EngineKind::WS)
    }

    fn consume(&mut self, event:Self::Event, buf: &mut VecDeque<Output>) {
        match event {
            LongPollEvent::GET => {},
            LongPollEvent::POST(msg) => {
                buf.push_back(Output::Receive(Payload::Message(msg)))
            }
        }
    }
}

pub struct Websocket {
}
pub enum WebsocketEvent {
    Ping,
    Pong
}

impl Transport for Websocket { 
    type Event = WebsocketEvent;

    fn consume(&mut self, event:Self::Event, buf: &mut VecDeque<Output>) {
        
    }
    
        
}

