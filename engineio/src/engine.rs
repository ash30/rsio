use std::time::Duration;
use std::time::Instant;
use std::fmt;
use crate::transport::TransportError;
pub use crate::proto::*;
use crate::transport::TransportKind;
use std::collections::VecDeque;

pub(crate) enum TransportType {
    LongPoll,
    Websocket
}

impl fmt::Display for TransportType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::LongPoll => "long_poll",
            Self::Websocket => "websocket"
        };
        write!(f,"{}",s)
    }
}

#[trait_variant::make(AsyncTransport: Send)]
pub(crate) trait AsyncLocalTransport {
    async fn engine_state_update(&mut self, next_state:EngineState) -> Result<Option<Vec<u8>>,TransportError>;
    async fn recv(&mut self) -> Result<Payload,TransportError>;
    async fn send(&mut self, data:Payload) -> Result<(),TransportError>;
    fn upgrades() -> Vec<String> {
        vec![]
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct OpenMessage {
    pub sid:Sid,
    pub upgrades: Vec<String>,
    #[serde(rename = "maxPayload")]
    pub max_payload:u64,
    #[serde(rename = "pingInterval")]
    pub ping_interval:u64,
    #[serde(rename = "pingTimeout")]
    pub ping_timeout:u64,
}

pub(crate) fn default_server_state_update<T:AsyncLocalTransport>(transport:&T, sid:Sid, config:TransportConfig, next_state:EngineState) -> Option<Payload>{
    match next_state {
        EngineState::Connecting(_) => { 
            let data = OpenMessage {
                sid,
                upgrades: <T as AsyncLocalTransport>::upgrades(),
                max_payload: config.max_payload,
                ping_timeout: config.ping_timeout,
                ping_interval: config.ping_interval
            };
            Some(Payload::Open(serde_json::to_vec(&data).unwrap()))
        },
        EngineState::Connected(Heartbeat::Unknown(_),_,_) => Some(Payload::Ping),
        EngineState::Closing(t,r) => {
            match r {
                EngineCloseReason::ClientClose => Some(Payload::Noop),
                _ => Some(Payload::Close(r))
            }
        },
        _ => None
    }
}

pub(crate) fn default_client_state_update(prev_state:EngineState, next_state:EngineState) -> Option<Payload> {
    match next_state {
        EngineState::Closing(t,r) => Some(Payload::Close(EngineCloseReason::ServerClose)),
        EngineState::Closed(r) => Some(Payload::Noop),
        _ => None
    }
}

// ---------
fn default_time_processor(s:EngineState, now:Instant) -> Override<StateTransition> {
    let next_deadline = s.deadline()?;
    if now > next_deadline {
        match s {
            EngineState::Connecting(s) => Override(Some(EngineState::Closing(now, EngineCloseReason::Timeout))),
            EngineState::Connected(Heartbeat::Alive(_),c,s) => Override(Some(EngineState::Connected(Heartbeat::Unknown(now),c,s))),
            EngineState::Connected(Heartbeat::Unknown(_),_,_) => Override(Some(EngineState::Closing(now, EngineCloseReason::Timeout))),
            EngineState::Closing(start,r) => Override(Some(EngineState::Closed(r))),
            _ => None
        }
    }
    else {
        None
    }
}

fn default_recv_processor(p:&Payload, s:EngineState, t:Instant) -> Override<StateTransition> {
    match (p,s) {
        (Payload::Close(_), EngineState::Closed(_)) => None,
        (Payload::Close(_), EngineState::Closing(_,_)) => None,
        (Payload::Close(r), _) => Override(Some(EngineState::Closing(t, *r))),
        (_, EngineState::Connected(_,c,s)) => Override(Some(EngineState::Connected(Heartbeat::Alive(t),c,s))),
        _ => None
    }
}

fn default_transport_update_processor(r:&Result<Option<Vec<u8>>,TransportError>, s:EngineState, t:Instant) -> Override<StateTransition> {
    match (s,r) {
        (_, Err(e)) => Override(Some(EngineState::Closed(EngineCloseReason::Error(EngineError::Generic)))),
        (EngineState::New,_) => Override(Some(EngineState::Connecting(t))),
        (EngineState::Closing(_,reason),_) => Override(Some(EngineState::Closed(reason))),
        _ => None
    }
}

fn default_processor(i:&Input, s:EngineState, n:Instant) -> Override<StateTransition> {
    match i {
        Input::TransportUpdate(s,r) => default_transport_update_processor(&r,*s,n),
        Input::Recv(p) => default_recv_processor(p,s,n),
        Input::Time => default_time_processor(s,n),
        _ => None
    }
}

pub(crate) fn create_server_engine(sid:Sid, config:TransportConfig, now:Instant) -> impl Engine{
    BaseEngine::new(EngineState::Connecting(now), move |i:&Input,s:EngineState,n:Instant| { match i {
        Input::TransportUpdate(s, Ok(_)) => {
                match s {
                    EngineState::Connecting(_) => Override(Some(EngineState::Connected(Heartbeat::Alive(now), config, sid))),
                    _ => None
                }
            },
            _ => None
        }
        .or_else(|| default_processor(i, s, n))
    })
}

pub(crate) fn create_client_engine(now:Instant) -> impl Engine {
    BaseEngine::new(EngineState::New, move |i:&Input,ss:EngineState,now:Instant| {
        return match i {
            Input::TransportUpdate(EngineState::Connecting(_),Ok(data)) => {
                let open_data = data
                    .as_ref()
                    .and_then(|a| Payload::decode(a, TransportKind::Poll).ok())
                    .and_then(|a| if let Payload::Open(data) = a { Some(data) } else { None } )
                    .map(|a| serde_json::from_slice::<OpenMessage>(a.as_ref()));

                if let Some(Ok(d)) = open_data {
                    let sid = d.sid;
                    let config = TransportConfig::from(d);
                    Override(Some(EngineState::Connected(Heartbeat::Alive(now), config,sid)))
                }
                else { None }
            },
            _ => None
        }
        .or_else(|| default_processor(i, ss, now))
    })
}

pub(crate) enum Input<'a> {
    Time,
    Recv(&'a Payload),
    Send(&'a MessageData),
    TransportUpdate(EngineState, Result<Option<Vec<u8>>,TransportError>)
}

pub(crate) enum Output {
    StateUpdate(EngineState),
    Wait(Instant)
}
type Override<T> = Option<T>;
type StateTransition = Option<EngineState>;
fn Override<T>(a:T) -> Override<T> { Some(a) }

pub trait Engine {
    fn process(&mut self, i:Input, now:Instant) -> Result<(), EngineError>;
    fn poll(&mut self, now:Instant) -> Option<Output>;
    fn close(&mut self, now:Instant, reason:Option<EngineCloseReason>);
    fn is_closed(&self) -> bool;
    fn is_connected(&self) -> bool;
}

pub(crate) struct BaseEngine<F> {
    state: EngineState,
    output:VecDeque<Output>,
    input_processor: F
}

impl<F> Engine for BaseEngine<F> 
where F: Fn(&Input,EngineState,Instant) -> Override<StateTransition>
{
    fn close(&mut self, now:Instant, reason:Option<EngineCloseReason>) {
        match self.state {
            EngineState::Closing(_,_) => {},
            EngineState::Closed(r) => {},
            _ => {
                self.update_state(EngineState::Closing(now, reason.unwrap_or(EngineCloseReason::ServerClose)))
            }
        }
    }
    fn is_closed(&self) -> bool {
        match self.state {
            EngineState::Closed(_) => true,
            _ => false
        }
    }

    fn is_connected(&self) -> bool {
        match self.state {
            EngineState::Connected(..) => true,
            _ => false
        }
    }

    fn process(&mut self, i:Input, now:Instant) -> Result<(),EngineError> {
        let _ = match (&i,self.state) {
            (Input::Recv(_), EngineState::Connected(..))  => Ok(()),
            (Input::Recv(_), _) => Err(EngineError::Generic),
            (Input::Send(_), EngineState::Connected(..))  => Ok(()),
            (Input::Send(_), _) => Err(EngineError::Generic),
            _ => Ok(())
        }?;

        if let Some(Some(state)) = (self.input_processor)(&i,self.state, now) {
            self.update_state(state);
        }
        Ok(())
    }

    fn poll(&mut self, now:Instant) -> Option<Output> {
        let _ = self.process(Input::Time, now);
        self.output.pop_front()
            .or_else(|| self.state.deadline().map(|t|Output::Wait(t)))
    }
}

impl <F> BaseEngine<F> {
    fn new(initial_state:EngineState, input_processor:F) -> Self {
        let mut output = VecDeque::new();
        output.push_back(Output::StateUpdate(initial_state));

        Self {
            state:initial_state,
            output,
            input_processor
        }
    }

    fn update_state(&mut self, s:EngineState) {
        self.state = s;
        self.output.push_back(Output::StateUpdate(s))
    }
}

#[derive(Debug,Copy,Clone)]
pub enum Heartbeat {
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
pub(crate) enum EngineState {
    New,
    Connecting(Instant),
    Connected(Heartbeat, TransportConfig, Sid),
    Closing(Instant, EngineCloseReason),
    Closed(EngineCloseReason)
}

impl EngineState {
    pub(crate) fn deadline(&self) -> Option<Instant> {
        match self {
            EngineState::New => None,
            EngineState::Connecting(start) => Some(*start + Duration::from_millis(5000)),
            EngineState::Connected(Heartbeat::Alive(s), config,_) => Some(*s + Duration::from_millis(config.ping_interval)),
            EngineState::Connected(Heartbeat::Unknown(s), config,_) => Some(*s + Duration::from_millis(config.ping_timeout)),
            EngineState::Closing(start,_) => Some(*start + Duration::from_millis(5000)),
            EngineState::Closed(r) => None
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



