use std::time::{Instant, Duration};
use std::fmt;
use crate::{Either};
pub use crate::proto::*;
use std::collections::VecDeque;

// =======================

#[derive(Debug)]
pub enum EngineInput<T> {
    Control(T),
    Data(Result<Payload, PayloadDecodeError>),
}

#[derive(Debug, Clone)]
pub enum EngineIOServerCtrls {
    Close,
}

#[derive(Debug, Clone)]
pub enum EngineIOClientCtrls {
    New(TransportKind),
    Poll,
    Close
}

#[derive(Debug, Clone)]
pub(crate) enum IO {
    Connect(Option<Sid>),
    Close,
    Recv(Payload),
    Send(Payload),
    Wait(Instant)
}

// =======================


// =====================

#[derive(Debug)]
pub(crate) enum EngineState {
    New { start_time:Instant },
    Connecting { start_time:Instant },
    Connected(ConnectedState),
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
    type Sender;
    type Receiver;
    fn time(&self, now:Instant, config:&TransportConfig) -> Option<EngineState>;
    fn send(&self, input:&EngineInput<Self::Sender>, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>, EngineError>;
    fn recv(&self, input:&Either<EngineInput<Self::Receiver>, TransportError>, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>, EngineError>;
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

    pub fn input(&mut self, i:Either<EngineInput<T::Sender>, Either<EngineInput<T::Receiver>, TransportError>>, now:Instant, config:&TransportConfig) -> Result<(),EngineError> {
        self.advance_time(now, config);
        let next_state = match &i {
            Either::A(a) => self.state.send(a, now, config),
            Either::B(b) => self.state.recv(b, now, config)
        };
        match next_state {
            Err(e) => {
                Err(e)
            },
            Ok(next_state) => {
                // Buffer Data Input AND action state transitions
                match i {
                    Either::A(EngineInput::Data(Ok(msg))) => { self.output.push_back(IO::Send(msg)) },
                    Either::B(Either::A(EngineInput::Data(Ok(msg)))) => { self.output.push_back(IO::Recv(msg)) }
                    _ => {}
                }
                if let Some(s) = next_state {
                    let e = self.state.update(s, &mut self.output, config).has_error();
                    if let Some(e) = e { Err(e.clone()) } else { Ok(()) }
                }
                else {
                    Ok(())
                }
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



