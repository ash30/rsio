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

enum EngineInput2<T> {
    Control(T),
    Data(Result<Payload,PayloadDecodeError>)
}

struct Stop();

enum IO_FOO<T>{
    Data(Payload),
    Signal(T),
}

pub(crate) trait Transport {
    type Signal;
    fn process_state_change(&mut self, next_state:EngineState, output: &mut VecDeque<IO>) -> Option<Stop>;

    // You can receive data - and then change state e.g. close, open, ping
    // You can receive signals ... POLL - this updates transport internal state
    // You ONLY receive transport specific signals, not OPEN or CLOSE 
    // You can send transport specific - Poll END etc 
    //
    // Engine specific open, close methods - sets state!
    // Initial state as part of constructor 
    // close, will need specific IO stuff 
    //
    fn process_recv(&mut self, input:IO_FOO<Self::Signal>, output: &mut VecDeque<Payload>) -> (Option<EngineState>, Option<Stop>) {
        (None, None)
    }
    fn process_send(&mut self, input:Payload, output: &mut VecDeque<IO_FOO<Self::Signal>>) -> (Option<EngineState>, Option<Stop>) {
        (None, None)
    }
}

struct FOO<T:Transport,Default:Transport>(T,Default);
impl <T:Transport,Default:Transport> Transport for FOO<T,Default> where Default:Transport<Signal = ()> {
    fn process_state_change(&mut self, next_state:EngineState, output: &mut VecDeque<IO>) -> Option<Stop> {
        self.0.process_state_change(next_state, output)
            .and_then(|_| self.1.process_state_change(next_state, output))
    }

    fn process_recv(&mut self, input:IO_FOO<Self::Signal>, output: &mut VecDeque<Payload>) -> (Option<EngineState>, Option<Stop>) {
        let (state, stop ) = self.0.process_recv(input, output);
        stop.is_none()
            .then(|_| if let IO_FOO::Data(p) = input { Some(IO_FOO::<()>::Data(p))} else { None })
            .map(|i| self.1.process_recv(i, output))
            .unwrap_or((state,stop))
    }

    fn process_send(&mut self, input:Payload, output: &mut VecDeque<IO_FOO<Self::Signal>>) -> (Option<EngineState>, Option<Stop>) {
        let (state, stop ) = self.0.process_send(input, output);
        stop.is_none()
            .then(|_| if let IO_FOO::Data(p) = input { Some(IO_FOO::<()>::Data(p))} else { None })
            .map(|i| self.1.process_send(i, output))
            .unwrap_or((state,stop))
    }
}

struct BaseServerTransport {}
impl Transport for BaseServerTransport {
    fn process_state_change(&mut self, next_state:EngineState, output: &mut VecDeque<IO>) -> Option<Stop> {
        match next_state {
            EngineState::New(s) => {},
            EngineState::Connected(h) => {
                // DISPATCH OPEN 
            },
            EngineState::Closing(t,r) => {
                // DISPATCH CLOSE
            }
            _ => {}
        }
    }

    fn process_recv(&mut self, input:IO_FOO<Self::Signal>, output: &mut VecDeque<Payload>) -> (Option<EngineState>, Option<Stop>) {
        output.push_back(input);
        output.push_back(input);
        let s = match input {
            IO_FOO::Data(Payload::Close(b)) => {
                Some(EngineState::Closing(Instant::now(), b))
            },
            _ => None
        };
        (None,None)
    }

    // Can you send CLOSE?
    // NO!!
    fn process_send(&mut self, input:Payload, output: &mut VecDeque<IO_FOO<Self::Signal>>) -> (Option<EngineState>, Option<Stop>) {
        output.push_back(input);
        (None, None)
    }
}

struct BaseClientTransport {}
impl Transport for BaseClientTransport {
    fn process_state_change(&mut self, next_state:EngineState, output: &mut VecDeque<IO>) -> Option<Stop> {
        match next_state {
            EngineState::New(s) => {},
            EngineState::Connected(h,c) => {
            },
            EngineState::Closing(t,r) => {
                // DISPATCH CLOSE
            }
            _ => {}
        }
    }

    fn process_recv(&mut self, input:IO_FOO<Self::Signal>, output: &mut VecDeque<Payload>) -> (Option<EngineState>, Option<Stop>) {
        // Session level filters...
        // We need sperate buffers ...
        output.push_back(input);
        let s = match input {
            IO_FOO::Data(Payload::Open(b)) => {
                // TODO: FIX inits
                Some(EngineState::Connected(Heartbeat::Alive(Instant::now()), TransportConfig::default()))
            },
            IO_FOO::Data(Payload::Close(b)) => {
                Some(EngineState::Closing(Instant::now(), b))
            },
            _ => None
        };
        (None,None)
    }

    fn process_send(&mut self, input:Payload, output: &mut VecDeque<IO_FOO<Self::Signal>>) -> (Option<EngineState>, Option<Stop>) {
        output.push_back(input);
        (None, None)
    }
}

pub struct GenericTransport();
impl Transport for GenericTransport {
    type Signal = ();
}

type ServerTransport<T> = FOO<T, BaseServerTransport>;
type ClientTransport<T> = FOO<T, BaseClientTransport>;

pub fn create_server_transport<T>(transport:T) -> ServerTransport<T> {
    FOO(transport, BaseServerTransport {  })
}
pub fn create_client_transport<T>(transport:T) -> ClientTransport<T> {
    FOO(transport, BaseClientTransport {  })
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
    Connected(Heartbeat, TransportConfig),
    Closing(Instant, EngineCloseReason),
    Closed(EngineCloseReason)
}

#[derive(Debug)]
pub(crate) struct Engine<T:Transport> {
    recv_buffer: VecDeque<Payload>,
    send_buffer: VecDeque<IO_FOO<T::Signal>>,
    state:EngineState,
    transport:T
}


/* The engine is a state machine keeping track of connectivtiy
 * Transports map their ingress into Generalised `EngineInput`
 * but we still need a specialised part of the statemachine.
 * For example, Polling has rules around Poll attempts which requires state 
 * so statemachine holds specific polling validation to ensure input is valid.
 */

impl <T> Engine<T> where T: Transport 
{
    pub fn new(now:Instant, transport:T, initial_state:EngineState) -> Self {
        Self {
            recv_buffer: VecDeque::new(),
            send_buffer: VecDeque::new(),
            state: initial_state,
            transport
        }
    }

    // TODO: We need to return result !!!
    pub fn send(&mut self, input:Payload, now:Instant) -> Result<(),EngineError> {
        self.advance_time(now);
        self.transport.process_recv(input, &mut self.send_buffer)
    }
    pub fn recv(&mut self, input:IO_FOO<T::Signal>, now:Instant) -> Result<(),EngineError> {
        self.advance_time(now);
        self.transport.process_recv(input, &mut self.recv_buffer)
    }

    pub fn poll(&mut self, now:Instant, config:&TransportConfig) -> Option<IO> {
        self.advance_time(now);
        self.recv_buffer.pop_front()
            .or(self.send_buffer.pop_front())
            .or_else(|| self.next_deadline())
            .map(|t|IO::Wait(t))
    }

    fn next_deadline(&self) -> Option<Instant> {
        match self.state {
            EngineState::New(start) => Some(start + Duration::from_millis(5000)),
            EngineState::Connected(Heartbeat::Alive(s), config) => Some(s + Duration::from_millis(config.ping_interval)),
            EngineState::Connected(Heartbeat::Unknown(s), config) => Some(s + Duration::from_millis(config.ping_timeout)),
            EngineState::Closing(start,_) => Some(start + Duration::from_millis(5000)),
            EngineState::Closed(r) => None
        }
    }

    fn advance_time(&mut self, now:Instant) {
        let deadline = self.next_deadline();
        if now > deadline {
            let next = match self.state {
                EngineState::New(s) => Some(EngineState::Closing(now, EngineCloseReason::Timeout)),
                EngineState::Connected(Heartbeat::Alive(_),c) => Some(EngineState::Connected(Heartbeat::Unknown(now), c)),
                EngineState::Connected(Heartbeat::Unknown(_),c) => Some(EngineState::Closing(now, EngineCloseReason::Timeout)),
                EngineState::Closing(start,r) => Some(EngineState::Closing(now, r)),
                EngineState::Closed(r) => None
            };

            if let Some(s) = next {
                self.transport.process_state_change(s, self.send_buffer);
                self.state = s;
            }
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
    Transport(TransportError)
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



