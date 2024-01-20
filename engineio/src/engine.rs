use std::{collections::VecDeque, u8, time::{Instant, Duration}};

pub use crate::proto::*;


#[derive(Debug, Clone)]
pub enum TransportState { 
    New,
    Connected { last_poll:Instant },
    Closed(EngineCloseReason)
}

#[derive(Debug, Clone)]
pub enum Participant {
    Client,
    Server
} 

#[derive(Debug, Clone)]
pub enum PollingState {
    Poll { active:Option<Duration>},
    Continuous ,
}

#[derive(Debug, Clone)]
pub enum EngineCloseReason {
    Error(EngineError),
    Timeout,
    Command(Participant)
}

#[derive(Debug, Clone)]
pub enum EngineInputError {
    OpenFailed,
    InvalidPoll,
    AlreadyClosed
}

#[derive(Debug, Clone)]
pub struct EngineState {
    transport: TransportState,
    polling: PollingState, 
    poll_timeout:Duration,
    poll_duration: Duration,
}

impl EngineState {
    fn new(now:Instant) -> Self {
        return Self {
            transport: TransportState::New,
            polling: PollingState::Poll { active: None },
            poll_timeout: Duration::from_secs(2),
            poll_duration: Duration::from_secs(10)
        }   
    }
}


pub struct Engine  { 
    pub session:Sid,
    output:VecDeque<EngineOutput>,
    poll_buffer: VecDeque<EngineOutput>,
    state: EngineState
}

impl Engine  
{ 
    pub fn new(sid:Sid) -> Self {
        return Self { 
            session: sid,
            output: VecDeque::new(),
            poll_buffer: VecDeque::new(),
            state: EngineState::new(Instant::now())
        }
    }

    pub fn poll_output(&mut self) -> Option<EngineOutput> { 
        self.output.pop_front()
    }

    fn update(now:Instant, input:EngineInput, currentState:&EngineState, nextState:&mut EngineState, poll_buf_length:usize) -> Result<Vec<EngineOutput>,EngineInputError> {

        let mut output = Vec::new();
        // Before Consuming event, make sure poll timeout is valid
        match &currentState.transport {
            TransportState::Connected { last_poll } => {
                if now > *last_poll + currentState.poll_timeout{
                    nextState.transport = TransportState::Closed(EngineCloseReason::Timeout);
                }
            }
            _ => {}
        };

        let res = match input {
            EngineInput::New(config, kind) => {
                match &currentState.transport {
                    TransportState::New => {
                        let config = config.unwrap_or(TransportConfig::default());
                        // Update timeouts
                        nextState.poll_timeout = Duration::from_millis(config.ping_timeout.into());
                        nextState.poll_duration= Duration::from_millis(config.ping_interval.into());
                        nextState.polling = match kind {
                            EngineKind::Continuous => PollingState::Continuous,
                            EngineKind::Poll => PollingState::Poll { active: None }
                        };
                        nextState.transport = TransportState::Connected { last_poll: now };
                        Ok(())
                    },
                    _ => {
                        // ITS AN ERROR TO INPUT NEW IF CLOSED OR CONNECTED!!
                        Err(EngineInputError::OpenFailed)
                    }
                }
            },
            EngineInput::Data(src,payload) => {
                match (src, payload) {
                    (_, Payload::Close) => {
                        nextState.transport = TransportState::Closed(EngineCloseReason::Command(Participant::Client));
                        Ok(())
                    },

                    (_, Payload::Upgrade) => Ok(()),

                    (Participant::Client,p) => {
                        output.push(EngineOutput::Data(Participant::Client, p));
                        Ok(())
                    },

                    (Participant::Server,p) => {
                        if let TransportState::Closed(..)= currentState.transport {
                            // DONT ALLOW SERVER TO SEND EVENT IF CLOSED 
                            Err(EngineInputError::AlreadyClosed)
                        } 
                        else {
                            output.push(EngineOutput::Data(Participant::Server, p));
                            Ok(())
                        }
                    }
                }
            },
            EngineInput::Tock => {

                match currentState.polling { 
                        // we MUST make sure only origial poll can udpate state 
                    PollingState::Poll { active:Some(duration) } => {
                        let is_complete = if let TransportState::Connected { last_poll } = currentState.transport { 
                            last_poll + currentState.poll_timeout > now 
                        }
                        else { true } ;

                        if is_complete == true {
                            nextState.polling = PollingState::Poll { active: None};
                        }
                    },

                    _ => {}
                };
                Ok(())
            },

            EngineInput::Poll => {
                // POLLs are one of the inputs that are independant of current connection state
                // BUT CAN mutate it 
                match currentState.polling {
                    PollingState::Poll { active:Some(..) } => {
                        nextState.transport = TransportState::Closed(EngineCloseReason::Error(EngineError::InvalidPollRequest));
                        Err(EngineInputError::InvalidPoll)
                    },

                    PollingState::Poll { active:None } => {
                        if poll_buf_length > 0 {
                            nextState.polling = PollingState::Poll { active: Some( Duration::from_secs(1)) };
                        }
                        else {
                            nextState.polling = PollingState::Poll { active: Some( currentState.poll_duration) }
                        }
                        Ok(())
                    },

                    PollingState::Continuous{ .. } => {
                        Ok(())
                    },

                }
            },
            EngineInput::Close(..) => {
                match currentState.transport {
                    TransportState::Closed(..) => {
                        // TODO: WE SHOULD ERROR 
                        Err(EngineInputError::AlreadyClosed)
                    },
                    _ => {
                        nextState.transport = TransportState::Closed(EngineCloseReason::Command(Participant::Server));
                        Ok(())
                    }
                }
            },
        };
        res.and(Ok(output))
    }
    
    pub fn consume(&mut self, input:EngineInput, now:Instant) -> Result<(), EngineInputError> {
        dbg!();
        let currentState = &self.state;
        let mut nextState = self.state.clone();

        // CALCULATE NEXT STATE
        let mut output = Engine::update(now, input, currentState, &mut nextState, self.poll_buffer.len())?;

        // FORWARD TO CORRECT BUFFER
        let output_buffer = if let PollingState::Poll { .. } = nextState.polling { &mut self.poll_buffer } else { &mut self.output};
        output.drain(0..).for_each(|p| output_buffer.push_back(p));
        

        // WORK OUT DISPATCH
        match (&currentState.polling, &nextState.polling) { 
            // FINISHED POLLING - DRAIN BUFFER
            (PollingState::Poll { active:Some(..)},PollingState::Poll { active:None }) => {
                self.poll_buffer.drain(0..).for_each(|p| self.output.push_back(p));
                self.output.push_back(EngineOutput::ResetIO(Participant::Client));
            }

            (PollingState::Poll { active:None},PollingState::Poll { active:Some(..) }) => {
                self.output.push_back(EngineOutput::StoreIO(Participant::Client));
            }

            // UPDATED POLL DURATION
            (PollingState::Poll { active:Some(old) }, PollingState::Poll { active:Some(new) } )=> {
                self.output.push_back(
                    EngineOutput::Tick { length: *new }
                )
            }
            _ => {}
        };

        match (&currentState.transport, &nextState.transport) {
            (TransportState::Closed(..), TransportState::Closed(..)) => {},

            // Initial Transition to Closed 
            (_, TransportState::Closed(..)) => {
                if let PollingState::Poll { .. } = &nextState.polling  {
                    self.poll_buffer.drain(0..).for_each(|p| self.output.push_back(p));
                };
                self.output.push_back(EngineOutput::Data(Participant::Client, Payload::Close));
            }
            // Intial setup 
            (TransportState::New, TransportState::Connected { .. } ) => {
                let upgrades = if let PollingState::Continuous = nextState.polling { vec![] } else { vec!["websocket"] };
                let data = serde_json::json!({
                    "upgrades": upgrades,
                    //"maxPayload": config.max_payload,
                    "pingInterval": nextState.poll_duration,
                    "pingTimeout": nextState.poll_timeout,
                    "sid":  self.session
                });
                self.output.push_back(
                    EngineOutput::Data(
                        Participant::Server, Payload::Open(serde_json::to_vec(&data).unwrap())
                    )
                );

                if let PollingState::Continuous = nextState.polling {
                    self.output.push_back(
                        EngineOutput::StoreIO(Participant::Server)
                    );
                }

                // START TICK TOCK 
                self.output.push_back(
                    EngineOutput::Tick { length: nextState.poll_timeout }
                )
            }

            // UPDATED LAST POLL
            (TransportState::Connected { last_poll}, TransportState::Connected { last_poll:new_last_poll }) => {
                if new_last_poll > last_poll {
                    self.output.push_back(
                        EngineOutput::Tick { length: nextState.poll_timeout }
                    )
                }
            }
            _ => {}
        };


        self.state = nextState;
        return Ok(())
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

