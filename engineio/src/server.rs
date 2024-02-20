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
    EngineError,
    EngineState,
    ConnectedState

}, PollingState, EngineStateEntity};
use crate::proto::{Payload, TransportConfig, Sid};

pub (crate) struct EngineIOServer(EngineState, Sid);

impl EngineIOServer {
    pub fn new(sid:Sid,now:Instant) -> Self {
       return Self(EngineState::New { start_time: now }, sid)
    }
}

impl EngineStateEntity for EngineIOServer {
    type Sender = EngineIOServerCtrls;
    type Receiver = EngineIOClientCtrls;

    fn time(&self, now:Instant, config:&TransportConfig) -> Option<EngineState> {
        match &self.0 {
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
            EngineState::Closed(_) => None,
            EngineState::Connecting { start_time } => None
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        match &self.0 {
            EngineState::New { start_time } => Some(*start_time + Duration::from_secs(5)),
            EngineState::Connected(ConnectedState(t,h,c)) => {
                let next_poll_deadline = if let Transport::Polling(PollingState { active:Some((start,length)), count }) = t { Some(*start + *length) } else { None };
                let next_heartbeat_deadline = if let Some(s) = h.last_ping { s + Duration::from_millis(c.ping_timeout) } else { h.last_seen + Duration::from_millis(c.ping_interval) };
                [ next_poll_deadline, Some(next_heartbeat_deadline) ].into_iter().filter_map(|d|d).min()
            }
            EngineState::Closed(..) => None,
            EngineState::Connecting { start_time } => todo!()
        }
    }

    fn send(&self, input:&EngineInput<Self::Sender>, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>, EngineError> {
        match input {
            EngineInput::Data(_) => {
                match &self.0 {
                    EngineState::Connected(state) => Ok(Some(EngineState::Connected(state.clone().update(|t,_| if let Transport::Polling(p) = t { p.increase_count(); })))),
                    _ => Err(EngineError::AlreadyClosed)
                }
            },

            EngineInput::Control(EngineIOServerCtrls::Close) => {
                match &self.0 {
                    EngineState::Connected(..) => Ok(Some(EngineState::Closed(EngineCloseReason::ServerClose))),
                    _ => Err(EngineError::AlreadyClosed)
                }
            }
            _ => Ok(None)
        }
    }

    fn recv(&self, input:&EngineInput<Self::Receiver>, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>, EngineError> {
        match &input {
            EngineInput::Data(Err(e)) => {
                Ok(EngineState::Closed(EngineCloseReason::Error(EngineError::UnknownPayload)))
            },
            EngineInput::Data(Ok(p)) => {
                match &self.0 {
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
                match &self.0 {
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
                match &self.0 {
                    EngineState::Connecting { start_time } => todo!(),
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
        }.map(|r| Some(r))
    }

    fn update(&mut self, next_state:EngineState, out_buffer:&mut VecDeque<EngineOutput>, config:&TransportConfig) -> &EngineState {
        match (&self.0, &next_state) {
            (EngineState::Closed(_), _ ) => {},

            (EngineState::New {..} , EngineState::Connected(ConnectedState(t,_,config))) => {
                let upgrades = if let Transport::Polling { .. } = t { vec!["websocket"] } else { vec![] };
                let data = serde_json::json!({
                    "upgrades": upgrades,
                    "maxPayload": config.max_payload,
                    "pingInterval": config.ping_interval,
                    "pingTimeout": config.ping_timeout,
                    "sid":  self.1
                });
                out_buffer.push_back(EngineOutput::Stream(true));
                out_buffer.push_back(EngineOutput::Send(Payload::Open(serde_json::to_vec(&data).unwrap())));
                if let Transport::Polling { .. } = t { 
                    out_buffer.push_back(EngineOutput::Stream(false));
                };
            },

            // Polling Start 
            (EngineState::Connected(ConnectedState(Transport::Polling(PollingState { active:None, .. }), _,_ )),
             EngineState::Connected(ConnectedState(Transport::Polling(PollingState { active:Some(..), ..}),_,_))) => { 
                out_buffer.push_back(EngineOutput::Stream(true));
            }
            
            // Polling End
            (EngineState::Connected(ConnectedState(Transport::Polling(PollingState { active:Some(..), count}),_,_)),
             EngineState::Connected(ConnectedState(Transport::Polling(PollingState { active:None, .. }),_,_))) => {
                if *count == 0 { out_buffer.push_back(EngineOutput::Send(Payload::Ping));}
                out_buffer.push_back(EngineOutput::Stream(false));
            },

            // websocket ping pong
            (EngineState::Connected(ConnectedState(Transport::Continuous, Heartbeat { last_ping:None, .. }, _ )),
            EngineState::Connected(ConnectedState(Transport::Continuous, Heartbeat { last_ping:Some(_), .. }, _))) => {
                out_buffer.push_back(EngineOutput::Send(Payload::Ping));
            },

            // Close
            (prev, EngineState::Closed(EngineCloseReason::ClientClose)) => {
                match prev { 
                    EngineState::Connected(ConnectedState(Transport::Polling(PollingState { active:Some(_), count }),_,_)) if *count == 0 => { 
                        out_buffer.push_back(EngineOutput::Send(Payload::Noop))
                    },
                    _ => {}
                }
                out_buffer.push_back(EngineOutput::Stream(false));
            },

            (_, EngineState::Closed(reason)) => {
                out_buffer.push_back(EngineOutput::Send(Payload::Close(reason.clone())));
                out_buffer.push_back(EngineOutput::Stream(false));
            },
            _ => {}
        };

        self.0 = next_state;
        return &self.0

        
    }
}


#[cfg(test)]
mod tests {
//     use super::*;
//     use crate::{Payload, MessageData};
// 
//     fn setup(now:Instant) -> EngineIOServer {
//         return EngineIOServer { output: VecDeque::new(), session:uuid::Uuid::new_v4(), state: EngineState::New { start_time: now } }
//     }
// 
//     fn next_time(now:Instant, then:u64) -> Instant { now + Duration::from_millis(then) }
// 
//     fn drain(now:Instant, engine:&mut EngineIOServer) -> Option<Duration> {
//         loop {
//             match engine.poll_output(now) {
//                 None => break None,
//                 Some(EngineOutput::Pending(d)) => break Some(d),
//                 _ => {}
//             }
//         }
//     }
// 
//     #[test]
//     fn on_poll_new() {
//         let time = Instant::now();
//         let mut e = setup(time);
//         let time_next = time +  Duration::from_millis(1);
//         let _ =  e.input_recv(EngineInput::Control(EngineIOClientCtrls::New(None, EngineKind::Poll)), time);
// 
//         let out = e.poll_output(time_next);
//         let res = if let Some(EngineOutput::Stream(true)) = out { true } else { false };
//         assert!(res, "res was {out:?}");
// 
//         let out = e.poll_output(time_next);
//         let res = if let Some(EngineOutput::Send(Payload::Open(..))) = out { true } else { false };
//         assert!(res, "res was {out:?}");
//     }
// 
//     #[test] 
//     fn on_ws_new() {
//         let time = Instant::now();
//         let mut e =  setup(time);
//         let time_next = time +  Duration::from_millis(1);
//         let _ = e.input_recv(EngineInput::Control(EngineIOClientCtrls::New(None, EngineKind::Continuous)), time);
// 
//         let out = e.poll_output(time_next);
//         let res = if let Some(EngineOutput::Stream(true)) = out { true } else { false };
//         assert!(res, "res was {out:?}");
// 
//         let out = e.poll_output(time_next);
//         let res = if let Some(EngineOutput::Send(Payload::Open(..))) = out { true } else { false };
//         assert!(res, "res was {out:?}");
// 
//         let out = e.poll_output(time_next);
//         let res = if let Some(EngineOutput::Pending(_))= out { true } else { false };
//         assert!(res, "res was {out:?}");
//     }
// 
//     #[test] 
//     fn on_poll_ctrl_empty() {
//         let now = Instant::now();
//         let mut engine = setup(now);
//         let _ = engine.input_recv(EngineInput::Control(EngineIOClientCtrls::New(None, EngineKind::Poll)), now);
//         
//         let later = next_time(now, 100);
//         let _ = engine.input_recv(EngineInput::Control(EngineIOClientCtrls::Poll), later);
//         drain(now, &mut engine);
// 
//         let last_seen = if let EngineState::Connected(ConnectedState(_,Heartbeat { last_seen, last_ping },_,)) = engine.state { Some(last_seen) } else { None };
//         assert!(last_seen.is_some());
//         assert!(last_seen.unwrap() == later);
//     }
// 
//     #[test] 
//     fn on_poll_ctrl_pendingEvent() {
//         let now = Instant::now();
//         let mut engine = setup(now);
//         let _ = engine.input_recv(EngineInput::Control(EngineIOClientCtrls::New(None, EngineKind::Poll)), now);
// 
//         engine.input_recv(EngineInput::Data(Ok(Payload::Message(MessageData::String(vec![])))), now);
// 
//         let later = next_time(now, 100);
//         let _ = engine.input_recv(EngineInput::Control(EngineIOClientCtrls::Poll), later);
//         drain(now, &mut engine);
// 
//         let last_seen = if let EngineState::Connected(ConnectedState(_,Heartbeat { last_seen, last_ping },_,)) = engine.state { Some(last_seen) } else { None };
//         assert!(last_seen.is_some());
//         assert!(last_seen.unwrap() == later);
//     }
}
