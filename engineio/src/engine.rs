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
    Active {id: uuid::Uuid, start:Instant, duration:Duration},
    Continuous
}

#[derive(Debug)]
pub enum EngineCloseReason {
    Error(EngineError),
    Timeout,
    Command(Participant)
}

pub enum EngineInputError {
    AlreadyClosed
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
    pub fn new(sid:Sid) -> Self {
        return Self { 
            session: sid,
            output: VecDeque::new(),
            poll_buffer: VecDeque::new(),
            transport: TransportState::New,
            polling: PollingState::Inactive { lastPoll: Some(Instant::now()) },
            poll_timeout: Duration::from_secs(30*10),
            poll_duration: Duration::from_secs(30),
        }
    }

    pub fn poll_output(&mut self) -> Option<EngineOutput> { 
        // Let them drain output even when closed, but onced empty - forever return None
        match (&self.transport, self.output.pop_front()) {
            (TransportState::Closed, Some(p)) => Some(p),
            (TransportState::Closed, None) => None,
            (_, Some(p)) => Some(p),
            (_, None) => {
                let (duration, poll_id) = match self.polling {
                    PollingState::Active { id, start, duration } => (duration, Some(id)), 
                    PollingState::Inactive { lastPoll } => (self.poll_timeout, None),
                    PollingState::Continuous => (std::time::Duration::from_secs(60*60), None)
                };
                Some(EngineOutput::Pending(duration, poll_id ))
            }
        }

    }
    
    pub fn consume(&mut self, data:EngineInput, now:Instant) -> Result<(), EngineInputError> {

        // Before Consuming event, make sure poll timeout is valid
        match (&self.transport, &self.polling) {
            (TransportState::Connected, PollingState::Inactive { lastPoll:Some(last) }) => {
                if now > (*last + self.poll_timeout) {
                    self.transport = TransportState::Closed;
                    self.output.push_back(EngineOutput::Closed(EngineCloseReason::Timeout));
                }
            }
            _ => {}
        };


        match (data, &mut self.transport, &mut self.polling) {
            (EngineInput::Poll(_id), transportState, PollingState::Active { id, start, duration }) => {
                // ERROR If we receive another poll request wwhilst active and IDs don't match!!
                if _id != *id {
                    self.transport = TransportState::Closed;
                    self.output.push_back(
                        EngineOutput::Closed(EngineCloseReason::Error(EngineError::InvalidPollRequest))
                    );
                    self.poll_buffer.push_back(
                        EngineOutput::Data(Participant::Server, Payload::Close(None))
                    );
                }
                else {
                    // DRAIN IF CONNECTED and TIME is PASSED OR ALREADY CLOSED, else DO NOTHING
                    match (now > (*start + *duration), transportState) {
                        (false, TransportState::Connected) => {},
                        (false, TransportState::New) => {},
                        _ => { 
                            self.polling = PollingState::Inactive { lastPoll: Some(now) };
                            self.poll_buffer.drain(0..).for_each(|p| self.output.push_back(p));
                        }
                    }
                }
                Ok(())
            },

            (_, TransportState::Closed, _) => {
                // GENERALLY We do NOTHING if ALREADY CLOSED!
                Err(EngineInputError::AlreadyClosed)
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
                self.transport = TransportState::Connected;

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
                );
                Ok(())
            },

            (EngineInput::New(..), _, _) => {
                // SHOULD WE RETURN AN ERROR ??
                Ok(())
            },

            (EngineInput::Close(participant),_,_) => {
                self.poll_buffer.drain(0..).for_each(|p| self.output.push_back(p));
                self.output.push_back(EngineOutput::Closed(EngineCloseReason::Command(Participant::Server)));
                self.transport = TransportState::Closed;
                Ok(())
            },

            (EngineInput::Error,_,_) => {
                Ok(())
            },

            // PollingState
            (EngineInput::Poll(id), _, PollingState::Inactive { .. }) => {
                // FLUSH buffer else starting polling
                if self.poll_buffer.len() > 0 {
                   self.poll_buffer.drain(0..).for_each(|p| self.output.push_back(p));
                   self.polling = PollingState::Inactive { lastPoll: Some(now) }
                }
                else {
                    self.polling = PollingState::Active { id, start: now, duration: self.poll_duration };
                };
                Ok(())
            },
            

            (EngineInput::Poll(..), _, PollingState::Continuous) => {
                self.polling = PollingState::Inactive { lastPoll: Some(now) };
                self.poll_buffer.drain(0..).for_each(|p| self.output.push_back(p));
                Ok(())
            },

            // Payloads
            //
            // TODO:THIS WILL ONLY FIRE IF NOT CONNECTED ... 
            (EngineInput::Data(_,Payload::Close(..)),_, _) => {
                self.transport = TransportState::Closed;
                self.output.push_back(
                    EngineOutput::Closed(EngineCloseReason::Command(Participant::Client))
                );
                Ok(())
            },

            (EngineInput::Data(Participant::Client, Payload::Upgrade), _, _) => {
                self.polling = PollingState::Continuous;
                Ok(())
            },

            (EngineInput::Data(Participant::Client, p),_,_) => {
                self.output.push_back(EngineOutput::Data(Participant::Client, p));
                Ok(())
            },

            // Buffer server emitted events depending on polling state
            
             
            (EngineInput::Data(Participant::Server, p),_,PollingState::Continuous) => {
                self.output.push_back(EngineOutput::Data(Participant::Server, p));
                Ok(())
            }
            (EngineInput::Data(Participant::Server, p),_,PollingState::Inactive { .. }) => {
                self.poll_buffer.push_back(EngineOutput::Data(Participant::Server, p));
                Ok(())
            }
            (EngineInput::Data(Participant::Server, p),_,PollingState::Active { id, start, mut duration }) => {
                self.poll_buffer.push_back(EngineOutput::Data(Participant::Server, p));
                self.polling = PollingState::Active { id:*id, start: *start, duration: duration.min(Duration::from_secs(1)) };
                Ok(())
            }
            
            // NOP
            (EngineInput::NOP, _, _) => {
                Ok(())
            },

            (EngineInput::Listen(..), _, _ ) => {
                Ok(())
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

