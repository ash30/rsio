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
    Poll { active:Option<(Instant,Duration)>},
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
    max_payload: u32,
}

impl EngineState {
    fn new(now:Instant) -> Self {
        return Self {
            transport: TransportState::New,
            polling: PollingState::Poll { active: None },
            poll_timeout: Duration::from_secs(2),
            poll_duration: Duration::from_secs(10),
            max_payload: 1000000
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
        dbg!(self.output.pop_front())
    }

    fn update(now:Instant, input:EngineInput, currentState:&EngineState, nextState:&mut EngineState, poll_buf_length:usize) -> Result<Vec<EngineOutput>,EngineInputError> {

        let mut output = Vec::new();
        // Before Consuming event, make sure poll timeout is valid
        let res = match input {
            EngineInput::Listen => Ok(()),

            EngineInput::New(config, kind) => {
                match &currentState.transport {
                    TransportState::New => {
                        let config = config.unwrap_or(TransportConfig::default());
                        // Update timeouts
                        nextState.poll_timeout = Duration::from_millis(config.ping_timeout.into());
                        nextState.poll_duration= Duration::from_millis(config.ping_interval.into());
                        nextState.max_payload = config.max_payload;
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
                    (_, Payload::Close(..)) => {
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
                    PollingState::Poll { active:Some((start, duration)) } => {
                        let is_complete = if let TransportState::Connected { last_poll } = currentState.transport { 
                            start + duration > now 
                        }
                        else { true } ;

                        if is_complete == true {
                            nextState.polling = PollingState::Poll { active: None};
                        }
                    },
                    PollingState::Poll { active:None } => {
                        if let TransportState::Connected { last_poll } = currentState.transport { 
                            if now > last_poll + currentState.poll_timeout {
                                nextState.transport = TransportState::Closed(EngineCloseReason::Timeout);
                            }
                        }
                    }

                    _ => {}
                };
                Ok(())
            },

            EngineInput::Poll => {
                // POLLs are one of the inputs that are independant of current connection state
                // BUT CAN mutate it 

                match (&currentState.polling, &currentState.transport) {
                    (PollingState::Poll { active:Some(..) }, TransportState::Closed(..)) => {
                        Err(EngineInputError::AlreadyClosed)
                    },
                    (PollingState::Poll { active:Some(..) }, _ ) => {
                        nextState.transport = TransportState::Closed(EngineCloseReason::Error(EngineError::InvalidPollRequest));
                        nextState.polling = PollingState::Poll { active: Some( (now,Duration::from_millis(1))) };
                        Err(EngineInputError::InvalidPoll)
                    },
                    (PollingState::Poll { active:None }, TransportState::Connected { last_poll }) => {
                        if poll_buf_length > 0 {
                            nextState.polling = PollingState::Poll { active: Some( (now,Duration::from_secs(1))) };
                        }
                        else {
                            nextState.polling = PollingState::Poll { active: Some( (now,currentState.poll_duration)) };
                        }
                        Ok(())
                    },
                    (PollingState::Poll { active:None }, TransportState::Closed(..)) => Err(EngineInputError::AlreadyClosed),
                    _ =>  Err(EngineInputError::InvalidPoll),
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
        dbg!(&input);
        let currentState = &self.state;
        let mut nextState = self.state.clone();


        // Special case LISTEN ... Is there a better way?
        if let EngineInput::Listen = &input {
            self.output.push_back(EngineOutput::SetIO(Participant::Server, true));
        }

        // CALCULATE NEXT STATE
        let mut output = Engine::update(now, input, currentState, &mut nextState, self.poll_buffer.len());

        dbg!(&nextState);

        // FORWARD TO CORRECT BUFFER
        let err = match output {
            Ok(mut output) => {
                output.drain(0..).for_each(|p| { 
                    let output_buffer = if let PollingState::Poll { .. } = nextState.polling { &mut self.poll_buffer } else { &mut self.output};
                    match &p {
                        EngineOutput::Data(Participant::Server, d ) => output_buffer.push_back(p),
                        EngineOutput::Data(Participant::Client,d ) => self.output.push_back(p),
                        _ => {},
                    }
                });
                None
            },
            Err(e) => Some(e)
        };

        // WORK OUT DISPATCH
        match (&currentState.polling, &nextState.polling) { 
            // FINISHED POLLING - DRAIN BUFFER
            (PollingState::Poll { active:Some(..)},PollingState::Poll { active:None }) => {
                self.poll_buffer.drain(0..).for_each(|p| self.output.push_back(p));
                self.output.push_back(EngineOutput::SetIO(Participant::Client, false));
            }

            (PollingState::Poll { active:None},PollingState::Poll { active:Some(..) }) => {
                self.output.push_back(EngineOutput::SetIO(Participant::Client, true));
            }

            // UPDATED POLL DURATION
            (PollingState::Poll { active:Some((old_start,old_length)) }, PollingState::Poll { active:Some((new_start,new_length)) } )=> {
                if old_length != new_length {
                    self.output.push_back(
                        EngineOutput::Tick { length: *new_length }
                    )
                }
            }
            _ => {}
        };

        match (&currentState.transport, &nextState.transport) {
            (TransportState::Closed(..), TransportState::Closed(..)) => {},

            // Initial Transition to Closed 
            (_, TransportState::Closed(reason)) => {
                if let PollingState::Poll { .. } = &nextState.polling  {
                    self.poll_buffer.drain(0..).for_each(|p| self.output.push_back(p));
                };
                dbg!();
                self.output.push_back(EngineOutput::Data(Participant::Server, Payload::Close(reason.clone())));
                self.output.push_back(EngineOutput::SetIO(Participant::Client, false));
            }
            // Intial setup 
            (TransportState::New, TransportState::Connected { .. } ) => {
                let upgrades = if let PollingState::Continuous = nextState.polling { vec![] } else { vec!["websocket"] };
                let data = serde_json::json!({
                    "upgrades": upgrades,
                    "maxPayload": nextState.max_payload,
                    "pingInterval": nextState.poll_duration.as_millis(),
                    "pingTimeout": nextState.poll_timeout.as_millis(),
                    "sid":  self.session
                });
                self.output.push_back(
                    EngineOutput::SetIO(Participant::Client, true)
                );

                self.output.push_back(
                    EngineOutput::Data(
                        Participant::Server, Payload::Open(serde_json::to_vec(&data).unwrap())
                    )
                );

                if let PollingState::Poll {..} = nextState.polling {
                    self.output.push_back(
                        EngineOutput::SetIO(Participant::Client, false)
                    );
                }

                // START TICK TOCK 
                self.output.push_back(
                    EngineOutput::Tick { length: nextState.poll_timeout }
                )
            }

            // UPDATED LAST POLL
            (TransportState::Connected { last_poll}, TransportState::Connected { last_poll:new_last_poll }) => {
                if new_last_poll != last_poll {
                    self.output.push_back(
                        EngineOutput::Tick { length: nextState.poll_timeout }
                    )
                }
            }
            _ => {}
        };


        self.state = nextState;
        return if let Some(e) = err { Err(e) } else { Ok(()) }
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

