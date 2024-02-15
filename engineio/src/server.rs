use std::{collections::VecDeque, time::{Instant, Duration}};
use crate::{engine::{
    Heartbeat,
    Transport,
    EngineIOClientCtrls,
    EngineIOServerCtrls,
    EngineOutput,
    EngineInput,
    EngineKind,
    EngineCloseReason,
    EngineError
}, PollingState};
use crate::proto::{Payload, TransportConfig, Sid};

#[derive(Debug)]
enum EngineState {
    New { start_time:Instant } , 
    Connected(ConnectedState),
    Closed(EngineCloseReason),
}

impl EngineState {
    fn has_error(&self) -> Option<&EngineError> {
        match self {
            EngineState::Closed(EngineCloseReason::Error(e)) => Some(e),
            _ => None
        }
    }
}

#[derive(Debug, Clone)]
struct ConnectedState(Transport, Heartbeat, TransportConfig);

impl ConnectedState {
        fn new(t:Transport, config:Option<TransportConfig>, now:Instant) -> Self {
            return Self (t, Heartbeat { last_seen: now, last_ping: None }, config.unwrap_or(TransportConfig::default()))
        }

        fn update(mut self, f:impl Fn(&mut Transport, &mut Heartbeat) -> ()) -> Self {
            f(&mut self.0, &mut self.1);
            self
        }

        fn replace(mut self, f:impl Fn(Transport,Heartbeat) -> (Transport, Heartbeat)) -> Self {
            let (t,h) = f(self.0, self.1);
            return ConnectedState (t,h,self.2)
        }

        fn set_config(mut self, config:TransportConfig) -> Self {
            self.2 = config;
            return self
        }   
}

impl Default for ConnectedState {
    fn default() -> Self {
        return Self(Transport::Continuous, Heartbeat { last_seen: Instant::now(), last_ping: None }, TransportConfig::default())
    }
}

#[derive(Debug)]
pub(crate) struct EngineIOServer {
    pub session: Sid,
    output: VecDeque<EngineOutput>,
    state: EngineState,
}


impl EngineIOServer {
    pub fn new(id:Sid, now:Instant) -> Self {
        return Self { 
            session: id,
            output: VecDeque::new(),
            state: EngineState::New { start_time: now }
        }
    }

    pub fn input_send(&mut self, input:EngineInput<EngineIOServerCtrls>, now:Instant) -> Result<(),EngineError> {
        // For server send data, if invalid input, we return early because no 
        // additional state transition can happen 
        let next_state = match &input {
            EngineInput::Data(_) => {
                match &self.state {
                    EngineState::Connected(state) => Ok(Some(EngineState::Connected(state.clone().update(|t,_| if let Transport::Polling(p) = t { p.increase_count(); })))),
                    _ => Err(EngineError::AlreadyClosed)
                }
            },

            EngineInput::Control(EngineIOServerCtrls::Close) => {
                match &self.state {
                    EngineState::Connected(..) => Ok(Some(EngineState::Closed(EngineCloseReason::ServerClose))),
                    _ => Err(EngineError::AlreadyClosed)
                }
            }
            _ => Ok(None)
        }?;

        if let EngineInput::Data(Ok(p)) = input {
            self.output.push_back(EngineOutput::Send(p));
        }
        // We want to send data depending on state changes
        if let Some(s) = next_state { self.update_state(s) }
        Ok(())
    }

    pub fn input_recv(&mut self, input:EngineInput<EngineIOClientCtrls>, now:Instant) -> Result<(),EngineError> {
        // Invalid Input data from client CAN transition state to closed 
        // so we must compute outputs before returning result 
        let next = match &input {
            EngineInput::Data(Err(e)) => {
                Ok(EngineState::Closed(EngineCloseReason::Error(EngineError::UnknownPayload)))
            },
            EngineInput::Data(Ok(p)) => {
                match &mut self.state {
                    EngineState::Connected(state) => {
                        match p {
                            Payload::Close(..) => Ok(EngineState::Closed(EngineCloseReason::ClientClose)),
                            _ => Ok(EngineState::Connected(state.clone().update(|_,heartbeat| heartbeat.seen_at(now)))
)
                        }
                    },
                    _ => {
                        Err(EngineError::AlreadyClosed)
                    }
                }
            },
            EngineInput::Control(EngineIOClientCtrls::Poll) => {
                match &self.state {
                    EngineState::Connected(s) => {
                        match &s.0 {
                            Transport::Polling(PollingState { active:Some(..), ..}) => Ok(EngineState::Closed(EngineCloseReason::Error(EngineError::InvalidPoll))),
                            Transport::Polling(PollingState{ active:None, ..}) => {
                                Ok(EngineState::Connected(s.clone().update(|t,h| {
                                    h.seen_at(now);
                                    t.poll_state().map(|p| p.activate_poll(now, Duration::from_millis(s.2.ping_timeout)));
                                })))
                            },
                            Transport::Continuous => Ok(EngineState::Closed(EngineCloseReason::Error(EngineError::InvalidPoll)))
                        }
                    },
                    _ => Err(EngineError::AlreadyClosed)
                }
            },
            // Transport has closed without sending payload
            EngineInput::Control(EngineIOClientCtrls::Close) => {
                Ok(EngineState::Closed(EngineCloseReason::ClientClose))
            },

            EngineInput::Control(EngineIOClientCtrls::New(config,kind)) => {
                match &self.state {
                    EngineState::New{ .. }  => { 
                        let t = match kind {
                            EngineKind::Poll => Transport::Polling(PollingState::default()),
                            EngineKind::Continuous => Transport::Continuous
                        };
                        Ok(EngineState::Connected(ConnectedState::new( t, *config, now)))
                    },
                    EngineState::Connected(..) => Ok(EngineState::Closed(EngineCloseReason::Error(EngineError::Generic))),
                    EngineState::Closed(e) => Err(EngineError::AlreadyClosed)
                }
            }
        }?;

        if let EngineInput::Data(Ok(Payload::Message(msg))) = input {
            self.output.push_back(EngineOutput::Recv(Payload::Message(msg)))
        };
        
        self.update_state(next);
        if let Some(e) = self.state.has_error() {
            Err(e.clone())
        }
        else {
            Ok(())
        }
    }

    pub fn poll_output(&mut self, now:Instant) -> Option<EngineOutput> {
        // We have to push forward the 'time' element of state so we can trigger timeouts
        let next = match &self.state {
            EngineState::Connected(ConnectedState(_, Heartbeat { last_ping:Some(i), .. }, config)) if now > *i + Duration::from_millis(config.ping_timeout) => {
                Some(EngineState::Closed(EngineCloseReason::Timeout))
            },
            EngineState::Connected(state) => {
                Some(EngineState::Connected(state.clone().update(|transport,heartbeat| {
                    if let None = heartbeat.last_ping {
                        if now > heartbeat.last_seen + Duration::from_millis(state.2.ping_interval) { heartbeat.pinged_at(now) };
                    }
                    if let Transport::Polling(p@PollingState { active:Some(..), .. }) = transport {
                        p.update_poll(now)
                    }
                })))
            },
            EngineState::New { start_time } if now > *start_time + Duration::from_secs(5) => Some(EngineState::Closed(EngineCloseReason::Timeout)),
            EngineState::New { .. } => None,
            EngineState::Closed(_) => None
        };
        if let Some(next) = next { self.update_state(next); };

        self.output.pop_front().or_else(||{
            match &self.state {
                EngineState::New { start_time } => Some(EngineOutput::Pending(now - *start_time + Duration::from_secs(5))),
                EngineState::Connected(ConnectedState(t,h,c)) => {
                    let next_poll_deadline = if let Transport::Polling(PollingState { active:Some((start,length)), count }) = t { Some(*start + *length) } else { None };
                    let next_heartbeat_deadline = if let Some(s) = h.last_ping { s + Duration::from_millis(c.ping_timeout) } else { h.last_seen + Duration::from_millis(c.ping_interval) };

                    let deadline = [ 
                        next_poll_deadline.map(|d| d - now ),
                        Some(next_heartbeat_deadline - now)
                    ];

                    dbg!(&deadline);

                    let d = deadline
                    .into_iter()
                    .filter_map(|a| a)
                    .min().unwrap();

                    Some(EngineOutput::Pending(d))
                }
                EngineState::Closed(..) => None
            }
        })
        // Return Next Pending Deadline OR NONE if engine is closed 
    }

    fn update_state(&mut self, next_state:EngineState) {
        dbg!((&self.state,&next_state));
        // We update state and push any additional output due to state transition 
        let io = match (&self.state, &next_state) {
            (EngineState::Closed(_), _ ) => {},

            (EngineState::New {..} , EngineState::Connected(ConnectedState(t,_,config))) => {
                let upgrades = if let Transport::Polling { .. } = t { vec!["websocket"] } else { vec![] };
                let data = serde_json::json!({
                    "upgrades": upgrades,
                    "maxPayload": config.max_payload,
                    "pingInterval": config.ping_interval,
                    "pingTimeout": config.ping_timeout,
                    "sid":  self.session
                });
                self.output.push_back(EngineOutput::Stream(true));
                self.output.push_back(EngineOutput::Send(Payload::Open(serde_json::to_vec(&data).unwrap())));
                if let Transport::Polling { .. } = t { 
                    self.output.push_back(EngineOutput::Stream(false));
                };
            },

            // Polling Start 
            (EngineState::Connected(ConnectedState(Transport::Polling(PollingState { active:None, .. }), _,_ )),
             EngineState::Connected(ConnectedState(Transport::Polling(PollingState { active:Some(..), ..}),_,_))) => { 
                self.output.push_back(EngineOutput::Stream(true));
            }
            
            // Polling End
            (EngineState::Connected(ConnectedState(Transport::Polling(PollingState { active:Some(..), ..}),_,_)),
             EngineState::Connected(ConnectedState(Transport::Polling(PollingState { active:None, .. }),_,_))) => {
                self.output.push_back(EngineOutput::Stream(false));
            },

            // Close
            (_, EngineState::Closed(EngineCloseReason::ClientClose)) => {
                self.output.push_back(EngineOutput::Stream(false));
            },

            (_, EngineState::Closed(reason)) => {
                self.output.push_back(EngineOutput::Send(Payload::Close(reason.clone())));
                self.output.push_back(EngineOutput::Stream(false));
            },
            _ => {}
        };

        self.state = next_state;
        return io

    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Payload, MessageData};

    fn setup(now:Instant) -> EngineIOServer {
        return EngineIOServer { output: VecDeque::new(), session:uuid::Uuid::new_v4(), state: EngineState::New { start_time: now } }
    }

    fn next_time(now:Instant, then:u64) -> Instant { now + Duration::from_millis(then) }

    fn drain(now:Instant, engine:&mut EngineIOServer) -> Option<Duration> {
        loop {
            match engine.poll_output(now) {
                None => break None,
                Some(EngineOutput::Pending(d)) => break Some(d),
                _ => {}
            }
        }
    }

    #[test]
    fn on_poll_new() {
        let time = Instant::now();
        let mut e = setup(time);
        let time_next = time +  Duration::from_millis(1);
        let _ =  e.input_recv(EngineInput::Control(EngineIOClientCtrls::New(None, EngineKind::Poll)), time);

        let out = e.poll_output(time_next);
        let res = if let Some(EngineOutput::Stream(true)) = out { true } else { false };
        assert!(res, "res was {out:?}");

        let out = e.poll_output(time_next);
        let res = if let Some(EngineOutput::Send(Payload::Open(..))) = out { true } else { false };
        assert!(res, "res was {out:?}");
    }

    #[test] 
    fn on_ws_new() {
        let time = Instant::now();
        let mut e =  setup(time);
        let time_next = time +  Duration::from_millis(1);
        let _ = e.input_recv(EngineInput::Control(EngineIOClientCtrls::New(None, EngineKind::Continuous)), time);

        let out = e.poll_output(time_next);
        let res = if let Some(EngineOutput::Stream(true)) = out { true } else { false };
        assert!(res, "res was {out:?}");

        let out = e.poll_output(time_next);
        let res = if let Some(EngineOutput::Send(Payload::Open(..))) = out { true } else { false };
        assert!(res, "res was {out:?}");

        let out = e.poll_output(time_next);
        let res = if let Some(EngineOutput::Pending(_))= out { true } else { false };
        assert!(res, "res was {out:?}");
    }

    #[test] 
    fn on_poll_ctrl_empty() {
        let now = Instant::now();
        let mut engine = setup(now);
        let _ = engine.input_recv(EngineInput::Control(EngineIOClientCtrls::New(None, EngineKind::Poll)), now);
        
        let later = next_time(now, 100);
        let _ = engine.input_recv(EngineInput::Control(EngineIOClientCtrls::Poll), later);
        drain(now, &mut engine);

        let last_seen = if let EngineState::Connected(ConnectedState(_,Heartbeat { last_seen, last_ping },_,)) = engine.state { Some(last_seen) } else { None };
        assert!(last_seen.is_some());
        assert!(last_seen.unwrap() == later);
    }

    #[test] 
    fn on_poll_ctrl_pendingEvent() {
        let now = Instant::now();
        let mut engine = setup(now);
        let _ = engine.input_recv(EngineInput::Control(EngineIOClientCtrls::New(None, EngineKind::Poll)), now);

        engine.input_recv(EngineInput::Data(Ok(Payload::Message(MessageData::String(vec![])))), now);

        let later = next_time(now, 100);
        let _ = engine.input_recv(EngineInput::Control(EngineIOClientCtrls::Poll), later);
        drain(now, &mut engine);

        let last_seen = if let EngineState::Connected(ConnectedState(_,Heartbeat { last_seen, last_ping },_,)) = engine.state { Some(last_seen) } else { None };
        assert!(last_seen.is_some());
        assert!(last_seen.unwrap() == later);
    }
}
