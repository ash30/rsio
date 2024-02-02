

use crate::{MessageData, Payload, PayloadDecodeError, TransportConfig, EngineKind, EngineError, Sid, EngineCloseReason };
use std::{collections::VecDeque, time::{Instant, Duration}};

pub enum Either<A,B> { 
    A(A),
    B(B)
}

struct Time();

pub enum EngineIOServerCtrls {
    Close,
    Noop
}

pub enum EngineIOClientCtrls {
    New(Option<TransportConfig>, EngineKind),
    Poll,
    Close
}

pub enum EngineInput<T> {
    Control(T),
    Data(Result<Payload, PayloadDecodeError>),
}


pub enum IO {
    Open,
    Close,
    Recv(MessageData),
    Send(Payload),
    Flush,
    Time(Duration),
}

// ============================================

enum Transport {
    Polling { active:Option<(Instant,Duration)> },
    Continuous
}

struct Heartbeat {
    last_seen:Instant,
    last_ping:Option<Instant>
}

// ============================================

enum EngineState {
    New, 
    Connected(Transport, Heartbeat),
    Closing,
    Closed(EngineCloseReason),
}

struct EngineIOServer {
    pub session: Sid,
    output: VecDeque<IO>,
    poll_buffer: VecDeque<IO>,
    state: EngineState,
}

impl EngineIOServer {
    pub fn input_send(&self, input:EngineInput<EngineIOServerCtrls>, now:Instant) -> Result<(),EngineError> {

        let is_polling = match self.state {
            EngineState::Connected(Transport::Polling{ .. }) => true,
            _ => false
        };

        let buffer = match is_polling {
            true => &mut self.poll_buffer,
            false => &mut self.output
        };

        let nextState = match input {
            EngineInput::Data(result) => {
                match self.state {
                    EngineState::Connected(..) => Ok(None),
                    _ => Err(EngineError::AlreadyClosed)
                }
            },

            EngineInput::Control(EngineIOServerCtrls::Close) => {
                match self.state {
                    EngineState::Connected(..) => Ok(Some(EngineState::Closed(EngineCloseReason::Command(crate::Participant::Server)))),
                    _ => Err(EngineError::AlreadyClosed)
                }
            }
            _ => Ok(None)
        }?;

        if let EngineInput::Data(Ok(p)) = input {
            buffer.push_back(IO::Send(p));
        }

        if let Some(out) = nextState.and_then(|s| self.update(s)) {
            buffer.push_back(out);
        }
        Ok(())
    }

    pub fn input_recv(&mut self, input:EngineInput<EngineIOClientCtrls>, now:Instant) -> Result<(),EngineError> {
        let next = match input {
            EngineInput::Data(Err(e)) => {
                Ok(EngineState::Closed(EngineCloseReason::Error(EngineError::UnknownPayload)))
            },
            EngineInput::Data(Ok(p)) => {
                match self.state {
                    EngineState::Connected(transport, Heartbeat) => {
                        match p {
                            Payload::Close(..) => Ok(EngineState::Closed(EngineCloseReason::Command(crate::Participant::Client))),
                            // Update hearbeat based on any input 
                            _ => Ok(EngineState::Connected(transport, Heartbeat { last_seen: now, last_ping: None}))

                        }
                    },
                    // IF we receive data at any other time... error 
                    _ => {
                        Err(EngineError::AlreadyClosed)
                    }

                }
            },
            EngineInput::Control(EngineIOClientCtrls::Poll) => {
                match self.state {
                    EngineState::Connected(transport, Heartbeat) => {
                        match transport {
                            Transport::Polling { active:Some(..) } => Err(EngineError::InvalidPoll),
                            Transport::Polling { active:None } => Ok(EngineState::Connected(Transport::Polling { active: Some((now,Duration::from_secs(1))) }, Heartbeat {last_seen:now, last_ping:None})),
                            Transport::Continuous => Err(EngineError::InvalidPoll)
                        }
                    },
                    _ => Err(EngineError::AlreadyClosed)
                }
            },
            // Transport has closed without sending payload
            EngineInput::Control(EngineIOClientCtrls::Close) => {
                Ok(EngineState::Closed(EngineCloseReason::Command(crate::Participant::Client)))
            },

            EngineInput::Control(EngineIOClientCtrls::New(config,kind)) => {}
            
        };

        Ok(())
    }

    fn poll_output(now:Instant) -> Option<IO> {

        None
    }

    fn update(&self, nextState:EngineState) -> Option<IO> {

    }
}


struct EngineIOClient{
    pub session:Sid,
    output: VecDeque<IO>,
    state: EngineState
}
