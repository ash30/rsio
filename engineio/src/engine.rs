use std::time::Duration;
use std::time::Instant;
use std::fmt;
use crate::transport::Connection;
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

#[derive(Debug)]
pub(crate) enum EngineState {
    New { start_time:Instant },
    Connecting { start_time:Instant },
    Connected(Connection),
    Closed(EngineCloseReason),
}

impl EngineState {
    pub fn has_error(&self) -> Option<&EngineError> {
        match self {
            EngineState::Closed(EngineCloseReason::Error(e)) => Some(e),
            _ => None
        }
    }
}

pub(crate) trait EngineStateEntity {
    fn time(&self, now:Instant, config:&TransportConfig) -> Option<EngineState>;
    fn send(&self, input:&EngineInput, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>, EngineError>;
    fn recv(&self, input:&EngineInput, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>, EngineError>;
    fn update(&mut self, next_state:EngineState, out_buffer:&mut VecDeque<IO>, config:&TransportConfig) -> &EngineState;
    fn next_deadline(&self, config:&TransportConfig) -> Option<Instant>;
}

// =====================


trait Transport {
    fn process_input(&mut self, input:EngineInput) -> Result<Option<EngineStateFOO>,TransportError>;
    fn process_state_change(&self, state:EngineStateFOO, output: &mut VecDeque<IO>);

}

#[derive(Debug)]
enum HeartbeatFoo {
    Alive(Instant),
    Unknown(Instant)
}

impl HeartbeatFoo {
    fn last_beat(&self) -> Instant {
        match self {
            Self::Alive(i) => *i,
            Self::Unknown(i) => *i
        }
    }
}

#[derive(Debug)]
enum EngineStateFOO {
    New(Instant),
    Connected(HeartbeatFoo),
    Closing(Instant, EngineCloseReason),
    Closed(EngineCloseReason)
}

#[derive(Debug)]
struct EngineFOO<T> {
    output: VecDeque<IO>,
    state:EngineStateFOO,
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

impl <T> EngineFOO<T> where T: Transport 
{
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
                EngineStateFOO::New(start) => Some(start + Duration::from_millis(5000)),
                EngineStateFOO::Connected(heartbeat) => Some(heartbeat.last_beat() + Duration::from_millis(config.ping_timeout)),
                EngineStateFOO::Closing(start,_) => Some(start + Duration::from_millis(5000)),
                EngineStateFOO::Closed(r) => None
            }
            .map(|t|IO::Wait(t))
        })       
    }


    fn process(&mut self, input:FOOBAR, config:&TransportConfig) ->Result<(),EngineError> {
        let next = match self.state {
            EngineStateFOO::New(start) => {
                match input { 
                    FOOBAR::Time(now) => {
                        if now > start + Duration::from_millis(5000) {
                            Ok(Some(EngineStateFOO::Closed(EngineCloseReason::ServerClose)))
                        } else { Ok(None) } 
                    },
                    FOOBAR::Send(s) => Err(EngineError::Generic),
                    FOOBAR::Recv(r) => {
                        // Transports decides IF INPUTS move the state machine forward
                        let n = self.transport.process_input(r)
                            .map_err(|_| EngineError::Generic)?;
                        Ok(n)
                    }
                }
            },
            EngineStateFOO::Connected(heartbeat) => {
                match input { 
                    FOOBAR::Time(now) => {
                        if now > heartbeat.last_beat() + Duration::from_millis(config.ping_interval) + Duration::from_millis(config.ping_timeout) {
                            Ok(Some(EngineStateFOO::Closing(now,EngineCloseReason::Timeout)))
                        }
                        else if now > heartbeat.last_beat() + Duration::from_millis(config.ping_interval) {
                            Ok(Some(EngineStateFOO::Connected(HeartbeatFoo::Unknown(heartbeat.last_beat()))))
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
                                let n = self.transport.process_input(i)
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
                                let n = self.transport.process_input(i)
                                    .map_err(|_| EngineError::Generic)?;
                                Ok(n)
                            }
                        }   
                    }
                }
            },
            EngineStateFOO::Closing(start,reason) => {
                match input { 
                    FOOBAR::Time(now) => {
                        if now > start + Duration::from_millis(5000) { Ok(Some(EngineStateFOO::Closed(reason))) }
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
            EngineStateFOO::Closed(r) => {
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
                let s = EngineStateFOO::Closed(EngineCloseReason::Error(e));
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



#[derive(Debug)]
pub (crate) struct Engine<T> {
    output: VecDeque<IO>,
    state: T,
}

impl<T> Engine<T>
where T:EngineStateEntity {
    pub fn new(initial_state:T) -> Self {
        return Self { 
            output: VecDeque ::new(),
            state: initial_state,
        }
    }
    fn advance_time(&mut self, now:Instant, config:&TransportConfig) {
        while let Some(d) = self.state.next_deadline(config) {
            if now < d { break };
            if let Some(s) = self.state.time(now, config) {
                self.state.update(s, &mut self.output, config);
            }
        }
    }
    pub fn send(&mut self, input:EngineInput, now:Instant, config:&TransportConfig) -> Result<(), EngineError> {
        self.advance_time(now, config);
        match self.state.send(&input,now,config) { 
            Err(e) => Err(e),
            Ok(s) => { 
                if let EngineInput::Data(Ok(p)) = input { self.output.push_back(IO::Send(p)) };
                let e = s.and_then(|s| self.state.update(s, &mut self.output, config).has_error());
                if let Some(e) = e { Err(e.clone()) } else { Ok(()) }
            } 
        }
    }
    pub fn recv(&mut self, input:EngineInput, now:Instant, config:&TransportConfig) -> Result<(), EngineError> {
        self.advance_time(now, config);
        match self.state.recv(&input,now,config){ 
            Err(e) => Err(e),
            Ok(s) => { 
                if let EngineInput::Data(Ok(p)) = input { self.output.push_back(IO::Recv(p)) };
                let e = s.and_then(|s| self.state.update(s, &mut self.output, config).has_error());
                if let Some(e) = e { Err(e.clone()) } else { Ok(()) }
            } 
        }
    }

    pub fn poll(&mut self,now:Instant, config:&TransportConfig) -> Option<IO> {
        self.advance_time(now, config);
        self.output.pop_front().or_else(||{
            self.state.next_deadline(config).map(|d| IO::Wait(d))
        })

    }

}

// =====================

#[derive(Debug, Clone)]
pub enum EngineCloseReason {
    Error(EngineError),
    Timeout,
    ServerClose,
    ClientClose
}

#[derive(Debug, Clone)]
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



