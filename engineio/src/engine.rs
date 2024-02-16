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

#[derive(Debug, Clone)]
pub(crate) enum EngineOutput {
    Stream(bool),
    Recv(Payload),
    Send(Payload),
    Pending(Duration)
}

// =======================

#[derive(Debug, Clone)]
pub(crate) enum Transport {
    Polling(PollingState),
    Continuous
}

impl Transport {
    pub(crate) fn poll_state(&mut self) -> Option<&mut PollingState> {
        match self {
            Transport::Polling(p) => Some(p),
            _ => None
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PollingState {
    pub active: Option<(Instant,Duration)>,
    pub count: u64
}

impl Default for PollingState {
    fn default() -> Self {
        return Self {
            active:None,
            count:0
        }
    }
}

impl PollingState {
    pub fn activate_poll(&mut self, start:Instant, max_length:Duration) {
        // TODO: We need to ensure min length > heartbeat
        self.count = self.count;
        let length = if self.count > 0 { Duration::from_millis(100) } else { max_length };
        self.active = Some((start,length));
    }

    pub fn update_poll(&mut self, now:Instant) {
        if let Some((start,length)) = self.active {
            if now > start + length { self.active = None; self.count = 0; }
        }
    }

    pub fn increase_count(&mut self) {
        if let Some((s,length)) = self.active {
            if length > Duration::from_millis(100) { self.active = Some((s,Duration::from_millis(100)));}
        }
        self.count += 1;
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Heartbeat {
    pub last_seen:Instant,
    pub last_ping:Option<Instant>
}

impl Heartbeat {
    pub fn seen_at(&mut self, at:Instant){
        self.last_seen = at;
        self.last_ping = None;
    }
    pub fn pinged_at(&mut self, at:Instant){
        self.last_ping = Some(at);
    }
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



