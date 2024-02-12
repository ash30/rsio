use std::time::{Instant, Duration};
use std::fmt;
pub use crate::proto::*;

#[derive(Debug,Clone,Copy)]
pub enum EngineKind {
    Poll,
    Continuous
}

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
    New(Option<TransportConfig>, EngineKind),
    Poll,
    Close
}

pub(crate) enum IO {
    Open,
    Close
}

#[derive(Debug, Clone)]
pub(crate) enum EngineDataOutput {
    Recv(Payload),
    Send(Payload),
    Pending(Duration)
}

// =======================

#[derive(Debug, Clone)]
pub(crate) enum Transport {
    Polling { active:Option<(Instant,Duration)> },
    Continuous
}

#[derive(Debug, Clone)]
pub(crate) struct Heartbeat {
    pub last_seen:Instant,
    pub last_ping:Option<Instant>
}

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



