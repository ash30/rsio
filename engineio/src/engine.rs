use std::{collections::VecDeque, u8};
pub use crate::proto::*;

pub enum Participant {
    Client,
    Server
}

enum PollingState {
    Inactive,
    Active,
    Continuous
}

pub struct Engine  { 
    pub session:Sid,
    output:VecDeque<EngineOutput>,

    // Engine State
    transport: TransportState,
    polling: PollingState

}

impl Engine  
{ 
    pub fn new() -> Self {
        return Self { 
            session: uuid::Uuid::new_v4(),
            output: VecDeque::new(),
            transport: TransportState::Connected,
            polling: PollingState::Inactive
        }
    }

    pub fn poll_output(&mut self) -> Option<EngineOutput> { 
        self.output.pop_front()
    }
    
    pub fn consume(&mut self, data:EngineInput) {
        match (data, &self.transport, &self.polling) {
            
            // Copnnection + Errors
            (_, TransportState::Closed, _) => {
                // Should we return some sort of result here??
            },

            (EngineInput::Error,_,_) => {

            },

            // PollingState
            (EngineInput::PollStart, _, PollingState::Inactive) => {
                self.polling = PollingState::Active;
                self.output.push_back(EngineOutput::Pending)
            },
            
            (EngineInput::PollStart, _, _) => {
                self.transport = TransportState::Closed;
                self.output.push_back(EngineOutput::Closed(Some(EngineError::InvalidPollRequest)))
            },

            (EngineInput::PollEnd, _, _ ) => {
                self.polling = PollingState::Inactive;
            }

            // Payloads
            (EngineInput::Data(_,Payload::Close(_)),_, _) => {
                self.transport = TransportState::Closed;
                self.output.push_back(EngineOutput::Closed(None));
            },

            (EngineInput::Data(Participant::Client, p),_,_) => {
                self.output.push_back(EngineOutput::Data(Participant::Client, p));
            }

            (EngineInput::Data(Participant::Server, p),_,_) => {
                self.output.push_back(EngineOutput::Data(Participant::Server, p));
            }
        }
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

