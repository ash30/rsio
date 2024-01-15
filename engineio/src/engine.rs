use std::{collections::VecDeque, u8, time::{Instant, Duration}};
pub use crate::proto::*;


#[derive(Debug)]
pub enum Participant {
    Client,
    Server
} 

#[derive(Debug)]
pub enum PollingState {
    Inactive {lastPoll:Option<Instant>},
    Active {start:Instant, duration:Duration},
    Continuous
}

pub struct Engine  { 
    pub session:Sid,
    output:VecDeque<EngineOutput>,
    poll_buffer: VecDeque<EngineOutput>,

    // Engine State
    transport: TransportState,
    pub polling: PollingState, 
    pub poll_timeout:Duration,
    pub poll_duration: Duration,



}

impl Engine  
{ 
    pub fn new() -> Self {
        return Self { 
            session: uuid::Uuid::new_v4(),
            output: VecDeque::new(),
            poll_buffer: VecDeque::new(),
            transport: TransportState::New,
            polling: PollingState::Inactive { lastPoll: Some(Instant::now()) },
            poll_timeout: Duration::from_secs(30*10),
            poll_duration: Duration::from_secs(30),
        }
    }

    pub fn poll_output(&mut self) -> EngineOutput { 
        return if let Some(p) = self.output.pop_front() { p } 
        else {
            let duration = match self.polling {
                PollingState::Active { start, duration } => duration, 
                PollingState::Inactive { lastPoll } => self.poll_timeout,
                PollingState::Continuous => std::time::Duration::from_secs(60*60)
            };
            EngineOutput::Pending(duration)
        }
    }
    
    pub fn consume(&mut self, data:EngineInput, now:Instant) {
        match self.polling {
            PollingState::Active { start, duration } => {
                if now > (start + duration) {
                    self.polling = PollingState::Inactive { lastPoll: Some(now) };
                    self.poll_buffer.drain(0..).for_each(|p| self.output.push_back(p));
                    self.output.push_back(EngineOutput::Data(Participant::Server, Payload::Noop))
                }
            },
            PollingState::Inactive { lastPoll:Some(last) } => {
                if now > (last + self.poll_timeout) {
                    self.output.push_back(EngineOutput::Closed(None));
                }
            }
            _ => {}
        }


        match (data, &self.transport, &self.polling) {
            // Copnnection + Errors
            (_, TransportState::Closed, _) => {
                // Should we return some sort of result here??
            },

            (EngineInput::New(config, kind),TransportState::New, _) => {
                let config = config.unwrap_or(TransportConfig::default());
            
                // Update timeouts
                self.poll_timeout = Duration::from_millis(config.ping_timeout.into());
                self.poll_duration = Duration::from_millis(config.ping_interval.into());
                self.polling = match kind {
                    EngineKind::Continuous => PollingState::Continuous,
                    EngineKind::Poll => PollingState::Inactive { lastPoll: Some(now) }
                };

                println!("interval: {}", self.poll_duration.as_millis());

                let upgrades = if let PollingState::Continuous = self.polling { vec![] } else { vec!["websocket"] };
                let data = serde_json::json!({
                    "upgrades": upgrades,
                    "maxPayload": config.max_payload,
                    "pingInterval": config.ping_interval,
                    "pingTimeout": config.ping_timeout,
                    "sid":  self.session
                });
                self.output.push_back(
                    EngineOutput::Data(
                        Participant::Server, Payload::Open(serde_json::to_vec(&data).unwrap())
                    )
                )
            },

            (EngineInput::New(..), _, _) => {
                // SHOULD WE RETURN AN ERROR ??
            },

            (EngineInput::Close(..),_,_) => {
                   self.poll_buffer.drain(0..).for_each(|p| self.output.push_back(p));
                   self.output.push_back(EngineOutput::Closed(None));
                   self.transport = TransportState::Closed;
            },

            (EngineInput::Error,_,_) => {

            },

            // PollingState
            (EngineInput::Poll, _, PollingState::Inactive { .. }) => {
                // FLUSH buffer else starting polling
                if self.poll_buffer.len() > 0 {
                   self.poll_buffer.drain(0..).for_each(|p| self.output.push_back(p));
                   self.polling = PollingState::Inactive { lastPoll: Some(now) }
                }
                else {
                    self.polling = PollingState::Active { start: now, duration: self.poll_duration };
                }
            },
            
            (EngineInput::Poll, _, _) => {
                self.transport = TransportState::Closed;
                self.output.push_back(EngineOutput::Closed(Some(EngineError::InvalidPollRequest)))
            },

            // Payloads
            (EngineInput::Data(_,Payload::Close(_)),_, _) => {
                self.transport = TransportState::Closed;
                self.output.push_back(EngineOutput::Closed(None));
            },

            (EngineInput::Data(Participant::Client, Payload::Upgrade), _, _) => {
                self.polling = PollingState::Continuous;
            },

            (EngineInput::Data(Participant::Client, p),_,_) => {
                self.output.push_back(EngineOutput::Data(Participant::Client, p));
            },

            // Buffer server emitted events depending on polling state
            
             
            (EngineInput::Data(Participant::Server, p),_,PollingState::Continuous) => {
                    self.output.push_back(EngineOutput::Data(Participant::Server, p));
            }
            (EngineInput::Data(Participant::Server, p),_,PollingState::Inactive { .. }) => {
                    self.poll_buffer.push_back(EngineOutput::Data(Participant::Server, p));
            }
            (EngineInput::Data(Participant::Server, p),_,PollingState::Active { start, duration }) => {
                    self.poll_buffer.push_back(EngineOutput::Data(Participant::Server, p));
                    self.polling = PollingState::Active { start: *start, duration: *duration.min(&Duration::from_secs(1)) }
            }

            // NOP
            (EngineInput::NOP, _, _) => {

            },

            (EngineInput::Listen, _, _ ) => {}

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

