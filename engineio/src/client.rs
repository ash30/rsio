use std::{collections::VecDeque, time::{Instant, Duration}};

use crate::engine::{EngineState, EngineSignal};
use crate::engine::EngineError;
use crate::engine::EngineCloseReason;
use crate::engine::EngineInput;
use crate::engine::IO;
use crate::engine::EngineStateEntity;
use crate::proto::TransportConfig;
use crate::proto::Sid;
use crate::transport::Transport;
use crate::transport::PollingState;
use crate::transport::Heartbeat;
use crate::transport::Connection;

pub (crate) struct EngineIOClient(EngineState, Option<Sid>);

impl EngineIOClient {
    pub fn new(now:Instant) -> Self {
       return Self(EngineState::New { start_time: now }, None)
    }
}

impl EngineStateEntity for EngineIOClient {
    fn time(&self, now:Instant, config:&TransportConfig) -> Option<EngineState> {
        match &self.0 {
            EngineState::Connected(Connection(_,Heartbeat::Unknown { since,.. })) if now > *since + Duration::from_millis(config.ping_timeout) => Some(EngineState::Closed(EngineCloseReason::Timeout)),
            EngineState::Connected(state) => {
                Some(EngineState::Connected(state.clone().update(|transport,heartbeat| {
                    if heartbeat.is_alive() && now >= heartbeat.last_beat() + Duration::from_millis(config.ping_interval){
                        *heartbeat = heartbeat.to_unknown(now)
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

    fn send(&self, input:&EngineInput, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>,EngineError> {
        match input {
            EngineInput::Data(Err(e)) => Err(EngineError::UnknownPayload),

            EngineInput::Data(Ok(p)) => {
                Ok(None)
            },
            EngineInput::Control(EngineSignal::Poll) => {
                match &self.0 {
                    EngineState::Connected(s@Connection(Transport::Polling(PollingState { active:None, count }),_)) => {
                        Ok(Some(EngineState::Connected(s.clone().update(|t,h| {
                            t.poll_state().map(|p| p.activate_poll(now, Duration::from_millis(config.ping_interval)));
                        }))))
                    },
                    _ => Err(EngineError::InvalidPoll)
                }
            },
            EngineInput::Control(EngineSignal::New(kind)) => {
                match &self.0 { 
                    EngineState::New { .. } => Ok(Some(EngineState::Connecting { start_time: now })),
                    _ => Err(EngineError::OpenFailed)
                }
            },
            EngineInput::Control(EngineSignal::Close) => {
                Ok(Some(EngineState::Closed(EngineCloseReason::ClientClose)))
            }
        }
    }

    fn recv(&self, input:&EngineInput, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>, EngineError> {
        match input {
            EngineInput::Control(EngineSignal::Close) => {
                match &self.0 {
                    EngineState::Connected(..) => Ok(Some(EngineState::Closed(EngineCloseReason::ServerClose))),
                    _ => Err(EngineError::AlreadyClosed)
                }
            }
            _ => Ok(None)

        }
    }

    fn update(&mut self, next_state:EngineState, out_buffer:&mut VecDeque<IO>, config:&TransportConfig) -> &EngineState {
        match (&self.0, &next_state) {
            _ => {}
        }
        self.0 = next_state;
        return &self.0
    }

    fn next_deadline(&self, config:&TransportConfig) -> Option<Instant> {
        match &self.0 {
            EngineState::New { start_time } => Some(*start_time + Duration::from_secs(5)),
            EngineState::Connected(Connection(t,h)) => {
                Some(Instant::now() + Duration::from_millis(config.ping_interval + config.ping_timeout))
            },
            EngineState::Closed(..) => None,
            EngineState::Connecting { .. } => todo!()
        }
    }

}


