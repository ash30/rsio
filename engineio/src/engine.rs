use std::time::Duration;
use std::time::Instant;
use std::fmt;
use crate::transport::TransportError;
use crate::transport::TransportKind;
pub use crate::proto::*;
use std::collections::VecDeque;

// =======================

#[derive(Debug)]
pub enum EngineInput {
    Control(EngineSignal),
    Data(Result<Payload, PayloadDecodeError>),
}

#[derive(Debug, Clone)]
pub enum EngineSignal {
    New(TransportKind),
    Poll,
    Close,
}

#[derive(Debug, Clone)]
pub(crate) enum IO {
    Recv(Payload),
    Send(Payload),
    Wait(Instant)
}

// =====================


pub(crate) trait Transport {
    fn process_input(&mut self, input:EngineSignal) -> Result<Option<EngineState>,TransportError>;
    fn process_state_change(&self, state:EngineState, output: &mut VecDeque<IO>);
}

pub struct GenericTransport {}
impl Transport for GenericTransport {
    fn process_input(&mut self, input:EngineSignal) -> Result<Option<EngineState>,TransportError> {
        todo!()    
    }

    fn process_state_change(&self, state:EngineState, output: &mut VecDeque<IO>) {
        todo!()    
    }
}

#[derive(Debug,Copy,Clone)]
enum Heartbeat {
    Alive(Instant),
    Unknown(Instant)
}

impl Heartbeat {
    fn last_beat(&self) -> Instant {
        match self {
            Self::Alive(i) => *i,
            Self::Unknown(i) => *i
        }
    }
}

#[derive(Debug,Copy,Clone)]
enum EngineState {
    New(Instant),
    Connected(Heartbeat),
    Closing(Instant, EngineCloseReason),
    Closed(EngineCloseReason)
}

#[derive(Debug)]
pub(crate) struct Engine<T> {
    output: VecDeque<IO>,
    state:EngineState,
    transport:T
}

enum FOOBAR {
    Send(EngineInput),
    Recv(EngineInput),
    Time(Instant)
}   

/* The engine is a state machine keeping track of connectivtiy
 * Transports map their ingress into Generalised `EngineInput`
 * but we still need a specialised part of the statemachine.
 * For example, Polling has rules around Poll attempts which requires state 
 * so statemachine holds specific polling validation to ensure input is valid.
 */

impl <T> Engine<T> where T: Transport 
{
    pub fn new(now:Instant, transport:T) -> Self {
       Self { output: VecDeque::new(), state: EngineState::New(now), transport } 
    }

    pub fn send(&mut self, input:EngineInput, now:Instant, config:&TransportConfig) -> Result<(),EngineError> {
        self.process(FOOBAR::Time(now), config);
        self.process(FOOBAR::Send(input), config)
    }
    pub fn recv(&mut self, input:EngineInput, now:Instant, config:&TransportConfig) -> Result<(),EngineError> {
        self.process(FOOBAR::Time(now), config);
        self.process(FOOBAR::Recv(input), config)
    }

    pub fn poll(&mut self, now:Instant, config:&TransportConfig) -> Option<IO> {
        self.process(FOOBAR::Time(now), config);
        self.output.pop_front().or_else(||{
            // Calculate next deadline
            match self.state {
                EngineState::New(start) => Some(start + Duration::from_millis(5000)),
                EngineState::Connected(heartbeat) => Some(heartbeat.last_beat() + Duration::from_millis(config.ping_timeout)),
                EngineState::Closing(start,_) => Some(start + Duration::from_millis(5000)),
                EngineState::Closed(r) => None
            }
            .map(|t|IO::Wait(t))
        })       
    }

    fn process(&mut self, input:FOOBAR, config:&TransportConfig) ->Result<(),EngineError> {
        let next = match self.state {
            EngineState::New(start) => {
                match input { 
                    FOOBAR::Time(now) => {
                        if now > start + Duration::from_millis(5000) {
                            Ok(Some(EngineState::Closed(EngineCloseReason::ServerClose)))
                        } else { Ok(None) } 
                    },
                    FOOBAR::Send(s) => Err(EngineError::Generic),
                    FOOBAR::Recv(r) => {
                        Ok(None)
                        // Transports decides IF INPUTS move the state machine forward
                        //let n = self.transport.process_input(r)
                        //    .map_err(|_| EngineError::Generic)?;
                        //Ok(n)
                    }
                }
            },
            EngineState::Connected(heartbeat) => {
                match input { 
                    FOOBAR::Time(now) => {
                        if now > heartbeat.last_beat() + Duration::from_millis(config.ping_interval) + Duration::from_millis(config.ping_timeout) {
                            Ok(Some(EngineState::Closing(now,EngineCloseReason::Timeout)))
                        }
                        else if now > heartbeat.last_beat() + Duration::from_millis(config.ping_interval) {
                            Ok(Some(EngineState::Connected(Heartbeat::Unknown(heartbeat.last_beat()))))
                        }
                        else {
                            Ok(None)
                        }
                    },
                    FOOBAR::Send(i) => {
                        match i {
                            EngineInput::Data(Ok(p)) => {
                                self.output.push_back(IO::Send(p));
                                Ok(None)
                            },
                            EngineInput::Data(Err(e)) => {
                                todo!();
                            }
                            EngineInput::Control(c) => {
                                let n = self.transport.process_input(c)
                                    .map_err(|_| EngineError::Generic)?;
                                Ok(n)
                            }
                        }   
                    }
                    FOOBAR::Recv(i) => {
                        match i {
                            EngineInput::Data(Ok(p)) => {
                                self.output.push_back(IO::Recv(p));
                                Ok(None)
                            },
                            EngineInput::Data(Err(e)) => {
                                Err(EngineError::UnknownPayload)
                            }
                            EngineInput::Control(c) => {
                                let n = self.transport.process_input(c)
                                    .map_err(|_| EngineError::Generic)?;
                                Ok(n)
                            }
                        }   
                    }
                }
            },
            EngineState::Closing(start,reason) => {
                match input { 
                    FOOBAR::Time(now) => {
                        if now > start + Duration::from_millis(5000) { Ok(Some(EngineState::Closed(reason))) }
                        else { Ok(None) }
                        // If N time passes, just close please
                    },
                    FOOBAR::Send(i) => Err(EngineError::AlreadyClosed),
                    FOOBAR::Recv(i) => {
                        // TODO
                        Err(EngineError::AlreadyClosed)
                    }
                }
            },
            EngineState::Closed(r) => {
                match input {
                    FOOBAR::Time(i) => Ok(None),
                    _ => Err(EngineError::AlreadyClosed)
                }
            }
        };
        
        // Update state machine state if required and return to caller 
        // wether input was valid
        match next {
            Err(e) if e.is_terminal() => {
                let s = EngineState::Closed(EngineCloseReason::Error(e));
                self.transport.process_state_change(s, &mut self.output);
                self.state = s;
                return Err(e)
            },
            Err(e) => Err(e),
            Ok(Some(s)) => {
                self.transport.process_state_change(s, &mut self.output);
                self.state = s;
                return Ok(())
            },
            Ok(None) => Ok(())
        }
    }
}




// =====================

#[derive(Debug, Copy, Clone)]
pub enum EngineCloseReason {
    Error(EngineError),
    Timeout,
    ServerClose,
    ClientClose
}

#[derive(Debug, Copy,Clone)]
pub enum EngineError {
    Generic,
    MissingSession,
    AlreadyClosed,
    OpenFailed,
    InvalidPoll,
    UnknownPayload,
}

impl EngineError {
    pub(crate) fn is_terminal(self) -> bool {
        match self {
            _ => true
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f,"EngineError: {self:?}")
    }
}



