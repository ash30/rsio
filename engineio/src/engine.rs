use std::time::Instant;
use std::fmt;
use crate::transport::Connection;
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

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f,"EngineError: {self:?}")
    }
}



