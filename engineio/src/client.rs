use crate::{MessageData, Payload, PayloadDecodeError, TransportConfig, EngineKind, EngineError, Sid, EngineCloseReason, Either };
use std::{collections::VecDeque, time::{Instant, Duration}};

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

#[derive(Debug)]
pub enum EngineInput<T> {
    Control(T),
    Data(Result<Payload, PayloadDecodeError>),
}

pub enum IO {
    Open,
    Close
}

#[derive(Debug, Clone)]
pub enum EngineDataOutput {
    Recv(Payload),
    Send(Payload),
    Pending(Duration)
}

// ============================================

#[derive(Debug, Clone)]
enum Transport {
    Polling { active:Option<(Instant,Duration)> },
    Continuous
}

#[derive(Debug, Clone)]
struct Heartbeat {
    last_seen:Instant,
    last_ping:Option<Instant>
}

// ============================================

#[derive(Debug)]
enum EngineState {
    New, 
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
pub struct EngineIOServer {
    pub session: Sid,
    output: VecDeque<EngineDataOutput>,
    poll_buffer: VecDeque<EngineDataOutput>,
    state: EngineState,
}

impl EngineIOServer {
    pub fn new(id:Sid) -> Self {
        return Self { 
            session: id,
            output: VecDeque::new(),
            poll_buffer: VecDeque::new(),
            state: EngineState::New
        }
    }

    pub fn input_send(&mut self, input:EngineInput<EngineIOServerCtrls>, now:Instant) -> Result<Option<IO>,EngineError> {

        let is_polling = match &self.state {
            EngineState::Connected(ConnectedState(Transport::Polling{ .. },_,_)) => true,
            _ => false
        };


        let next_state = match &input {
            EngineInput::Data(_) => {
                match &self.state {
                    EngineState::Connected(..) => Ok(None),
                    _ => Err(EngineError::AlreadyClosed)
                }
            },

            EngineInput::Control(EngineIOServerCtrls::Close) => {
                match &self.state {
                    EngineState::Connected(..) => Ok(Some(EngineState::Closed(EngineCloseReason::Command(crate::Participant::Server)))),
                    _ => Err(EngineError::AlreadyClosed)
                }
            }
            _ => Ok(None)
        }?;

        if let EngineInput::Data(Ok(p)) = input {
            let buffer = match is_polling {
                true => &mut self.poll_buffer,
                false => &mut self.output
            };
            buffer.push_back(EngineDataOutput::Send(p));
        }

        if let Some(s) = next_state { 
            let buffer = match is_polling {
                true => &mut self.poll_buffer,
                false => &mut self.output
            };

            EngineIOServer::update(self.session, &self.state, &s, &mut self.output);
            self.state = s;
        }
        Ok(None)
    }

    pub fn input_recv(&mut self, input:EngineInput<EngineIOClientCtrls>, now:Instant) -> Result<Option<IO>,EngineError> {
        let next = match &input {
            EngineInput::Data(Err(e)) => {
                Ok(EngineState::Closed(EngineCloseReason::Error(EngineError::UnknownPayload)))
            },
            EngineInput::Data(Ok(p)) => {
                match &self.state {
                    EngineState::Connected(state) => {
                        match p {
                            Payload::Close(..) => Ok(EngineState::Closed(EngineCloseReason::Command(crate::Participant::Client))),
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
                Ok(EngineState::Closed(EngineCloseReason::Command(crate::Participant::Client)))
            },

            EngineInput::Control(EngineIOClientCtrls::New(config,kind)) => {
                match &self.state {
                    EngineState::New => { 
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

        // IF ERR, return
        // else udpate 
        //
        
        if let EngineInput::Data(Ok(Payload::Message(msg))) = input {
            self.output.push_back(EngineDataOutput::Recv(msg))
        };
        
        EngineIOServer::update(self.session, &self.state, &next, &mut self.output);
        self.state = next;

        // DO WE NEED TO GUARD AGAINT RERUNs ?
        if let Some(e) = self.state.has_error() {
            Err(e.clone())
        }
        else {
            Ok(None)
        }
    }

    pub fn poll_output(&mut self, now:Instant) -> Option<EngineDataOutput> {
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

        if let Some(new_state) = next {
            EngineIOServer::update(self.session, &self.state, &new_state, &mut self.output);
            self.state = new_state;
        }
        self.output.pop_front().map(|o| Either::A(o)).unwrap_or(Either::B(Pending(Duration::from_secs(1))))
    }

    fn update(sid:Sid, currrent_state:&EngineState, nextState:&EngineState, buffer: &mut VecDeque<EngineDataOutput>) {
        match (currrent_state, nextState) {
            // Initial OPEN
            (EngineState::New, EngineState::Connected(ConnectedState(t,_,config))) => {
                let upgrades = if let Transport::Polling { .. } = t { vec!["websocket"] } else { vec![] };
                let data = serde_json::json!({
                    "upgrades": upgrades,
                    "maxPayload": config.max_payload,
                    "pingInterval": config.ping_interval,
                    "pingTimeout": config.ping_timeout,
                    "sid":  sid
                });
                buffer.push_back(EngineDataOutput::Send(Payload::Open(serde_json::to_vec(&data).unwrap())));
                if let Transport::Polling { .. } = t { 
                    buffer.push_back(EngineDataOutput::Close)
                }
            },

            // Polling Start 
            (EngineState::Connected(ConnectedState(Transport::Polling { active:None }, _,_ )),
             EngineState::Connected(ConnectedState(Transport::Polling { active: Some(..) },_,_))) => { 
                buffer.push_back(EngineDataOutput::Open);
            }
            
            // Polling End
            (EngineState::Connected(ConnectedState(Transport::Polling { active:Some(..)}, _,_ )),
             EngineState::Connected(ConnectedState(Transport::Polling { active: None },_,_))) => {
                buffer.push_back(EngineDataOutput::Close);
            },

            // Close
            (_, EngineState::Closed(EngineCloseReason::Command(crate::Participant::Client))) => {},
            (_, EngineState::Closed(reason)) => {
                buffer.push_back(EngineDataOutput::Send(Payload::Close(reason.clone())));
            },
            _ => {}
        }
    }
}


struct EngineIOClient{
    pub session:Sid,
    output: VecDeque<EngineDataOutput>,
    state: EngineState
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::Payload;

    fn setup() -> EngineIOServer {
        return EngineIOServer { output: VecDeque::new(), session:uuid::Uuid::new_v4(), state: EngineState::New, poll_buffer:VecDeque::new() }
    }

    #[test]
    fn on_poll_new() {
        let mut e =  setup();
        let time = Instant::now();
        let time_next = time +  Duration::from_millis(1);
        e.input_recv(EngineInput::Control(EngineIOClientCtrls::New(None, EngineKind::Poll)), time);

        let out = e.poll_output(time_next);
        let res = if let Some(EngineDataOutput::Send(Payload::Open(..))) = out { true } else { false };
        assert!(res);

        let out = e.poll_output(time_next);
        let res = if let Some(EngineDataOutput::Close)  = out { true } else { false };
        assert!(res);
    }

    #[test] 
    fn on_ws_new() {
        let mut e =  setup();
        let time = Instant::now();
        let time_next = time +  Duration::from_millis(1);
        let _ = e.input_recv(EngineInput::Control(EngineIOClientCtrls::New(None, EngineKind::Continuous)), time);

        let out = e.poll_output(time_next);
        let res = if let Some(EngineDataOutput::Send(Payload::Open(..))) = out { true } else { false };
        assert!(res);

        let out = e.poll_output(time_next);
        let res = if let None = out { true } else { false };
        assert!(res);
    }

}
