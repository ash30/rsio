use std::{collections::VecDeque, time::{Instant, Duration}};
use crate::engine::{
    Heartbeat,
    Transport,
    EngineIOClientCtrls,
    EngineIOServerCtrls,
    EngineOutput,
    EngineInput,
    EngineKind,
    EngineCloseReason,
    EngineError
};
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

        fn update_last_seen(mut self, now:Instant) -> Self {
            self.1.last_seen = now;
            self.1.last_ping = None;
            return self
        }

        fn update_new_ping(mut self, now:Instant) -> Self {
            self.1.last_ping = Some(now);
            return self
        }

        fn update_new_poll(mut self, now:Instant, length:Duration) -> Self {
            match self.0 {
                Transport::Continuous => {},
                Transport::Polling{ .. } => { self.0 = Transport::Polling{ active:Some((now, length)) }; }
            };
            return self
        }

        fn set_config(mut self, config:TransportConfig) -> Self {
            self.2 = config;
            return self
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
                    EngineState::Connected(..) => Ok(None),
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
                match &self.state {
                    EngineState::Connected(state) => {
                        match p {
                            Payload::Close(..) => Ok(EngineState::Closed(EngineCloseReason::ClientClose)),
                            _ => Ok(EngineState::Connected(state.clone().update_last_seen(now))
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
                        match s.0 {
                            Transport::Polling { active:Some(..) } => Ok(EngineState::Closed(EngineCloseReason::Error(EngineError::InvalidPoll))),
                            Transport::Polling { active:None } => Ok(EngineState::Connected(s.clone().update_new_poll(now, Duration::from_secs(1)))),
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
                            EngineKind::Poll => Transport::Polling { active: None },
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
            EngineState::Connected(ConnectedState(t,h,c)) => {
                let next = match t {
                    Transport::Polling { active:Some((start,length)) } =>  {
                        if now >= *start+*length {
                            Some(ConnectedState(Transport::Polling { active: None }, h.clone(), c.clone()))
                        }
                        else {
                            None
                        }
                    },
                    _  => None
                }.unwrap_or(ConnectedState(t.clone(),h.clone(),c.clone()));

                let next = match h.last_ping {
                    None => if now > h.last_seen + Duration::from_millis(c.ping_interval) { 
                        EngineState::Connected(next.update_new_ping(now)) } else { EngineState::Connected(ConnectedState(t.clone(),h.clone(),c.clone())) 
                    },
                    Some(start) => if now > start + Duration::from_millis(c.ping_timeout) { 
                        EngineState::Closed(EngineCloseReason::Timeout) } else { EngineState::Connected(ConnectedState(t.clone(),h.clone(),c.clone()))
                    }
                };
                Some(next)
            },
            _ => None
        };

        if let Some(new_state) = next { self.update_state(new_state) } 
        self.output.pop_front().or_else(||{
            match &self.state {
                EngineState::New { start_time } => Some(EngineOutput::Pending(now - *start_time + Duration::from_secs(5))),
                EngineState::Connected(ConnectedState(t,h,c)) => {
                    let next_poll_deadline = if let Transport::Polling { active:Some((start,length)) } = t { Some(*start + *length) } else { None };
                    let next_heartbeat_deadline = if let Some(s) = h.last_ping { s + Duration::from_millis(c.ping_timeout) } else { h.last_seen + Duration::from_millis(c.ping_interval) };

                    let deadline = [ 
                        next_poll_deadline.map(|d| d - now ),
                        Some(next_heartbeat_deadline - now)
                    ]
                    .into_iter()
                    .filter_map(|a| a)
                    .min().unwrap();

                    Some(EngineOutput::Pending(deadline))
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
            (EngineState::Connected(ConnectedState(Transport::Polling { active:None }, _,_ )),
             EngineState::Connected(ConnectedState(Transport::Polling { active: Some(..) },_,_))) => { 
                self.output.push_back(EngineOutput::Stream(true));
            }
            
            // Polling End
            (EngineState::Connected(ConnectedState(Transport::Polling { active:Some(..)}, _,_ )),
             EngineState::Connected(ConnectedState(Transport::Polling { active: None },_,_))) => {
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
    use crate::Payload;

    fn setup(now:Instant) -> EngineIOServer {
        return EngineIOServer { output: VecDeque::new(), session:uuid::Uuid::new_v4(), state: EngineState::New { start_time: now } }
    }

    #[test]
    fn on_poll_new() {
        let time = Instant::now();
        let mut e =  setup(time);
        let time_next = time +  Duration::from_millis(1);
        e.input_recv(EngineInput::Control(EngineIOClientCtrls::New(None, EngineKind::Poll)), time);

        let out = e.poll_output(time_next);
        let res = if let Some(EngineOutput::Send(Payload::Open(..))) = out { true } else { false };
        assert!(res);
    }

    #[test] 
    fn on_ws_new() {
        let time = Instant::now();
        let mut e =  setup(time);
        let time_next = time +  Duration::from_millis(1);
        let _ = e.input_recv(EngineInput::Control(EngineIOClientCtrls::New(None, EngineKind::Continuous)), time);

        let out = e.poll_output(time_next);
        let res = if let Some(EngineOutput::Send(Payload::Open(..))) = out { true } else { false };
        assert!(res);

        let out = e.poll_output(time_next);
        let res = if let None = out { true } else { false };
        assert!(res);
    }

}
