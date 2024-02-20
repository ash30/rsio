use std::time::{Instant, Duration};
use std::fmt;
use crate::Either;
pub use crate::proto::*;
use std::collections::VecDeque;

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
    New(EngineKind),
    Poll,
    Close
}

#[derive(Debug, Clone)]
pub(crate) enum EngineOutput {
    Stream(bool),
    Recv(Payload),
    Send(Payload),
    Wait(Instant)
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
    pub count: u64,
}

impl Default for PollingState {
    fn default() -> Self {
        return Self {
            active:None,
            count:0,
            //max_length: Duration::from_millis(TransportConfig::default().ping_interval),
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

// =====================
#[derive(Debug, Clone)]
pub(crate) struct ConnectedState(pub Transport, pub Heartbeat);

impl ConnectedState {
        pub fn new(t:Transport, now:Instant) -> Self {
            return Self (t, Heartbeat { last_seen: now, last_ping: None })
        }

        pub fn update(mut self, f:impl Fn(&mut Transport, &mut Heartbeat) -> ()) -> Self {
            f(&mut self.0, &mut self.1);
            self
        }
}


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
    fn recv(&self, input:&EngineInput<Self::Receiver>, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>, EngineError>;
    fn update(&mut self, next_state:EngineState, out_buffer:&mut VecDeque<EngineOutput>, config:&TransportConfig) -> &EngineState;
    fn next_deadline(&self, config:&TransportConfig) -> Option<Instant>;
}

// =====================

#[derive(Debug)]
pub (crate) struct Engine<T> {
    output: VecDeque<EngineOutput>,
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

    pub fn input(&mut self, i:Either<EngineInput<T::Sender>,EngineInput<T::Receiver>>, now:Instant, config:&TransportConfig) -> Result<(),EngineError> {
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
                    Either::A(EngineInput::Data(Ok(msg))) => { self.output.push_back(EngineOutput::Send(msg)) },
                    Either::B(EngineInput::Data(Ok(msg))) => { self.output.push_back(EngineOutput::Recv(msg)) }
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

    pub fn poll(&mut self,now:Instant, config:&TransportConfig) -> Option<EngineOutput> {
        let next_state = self.state.time(now, config);
        self.output.pop_front().or_else(||{
            self.advance_time(now, config);
            self.state.next_deadline(config).map(|d| EngineOutput::Wait(d))
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



