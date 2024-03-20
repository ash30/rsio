use std::{collections::VecDeque, time::{Instant, Duration}};

use crate::engine::EngineState;
use crate::engine::EngineError;
use crate::engine::EngineCloseReason;
use crate::engine::EngineInput;
use crate::engine::IO;
use crate::engine::EngineIOClientCtrls;
use crate::engine::EngineIOServerCtrls;
use crate::engine::EngineStateEntity;
use crate::engine::Engine;
use crate::transport::TransportKind;
use crate::proto::TransportConfig;
use crate::proto::Sid;
use crate::transport::Transport;
use crate::transport::PollingState;
use crate::transport::Connection;

pub (crate) struct EngineIOClient(EngineState, Option<Sid>);

impl EngineIOClient {
    pub fn new(now:Instant) -> Self {
       return Self(EngineState::New { start_time: now }, None)
    }
}

impl EngineStateEntity for EngineIOClient {
    type Receive = EngineIOServerCtrls;
    type Send = EngineIOClientCtrls;

    fn time(&self, now:Instant, config:&TransportConfig) -> Option<EngineState> {
        todo!()
    }

    fn send(&self, input:&EngineInput<EngineIOClientCtrls>, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>,EngineError> {
        match input {
            EngineInput::Data(Err(e)) => Err(EngineError::UnknownPayload),

            EngineInput::Data(Ok(p)) => {
                Ok(None)
            },
            EngineInput::Control(EngineIOClientCtrls::Poll) => {
                match &self.0 {
                    EngineState::Connected(s@Connection(Transport::Polling(PollingState { active:None, count }),_)) => {
                        Ok(Some(EngineState::Connected(s.clone().update(|t,h| {
                            t.poll_state().map(|p| p.activate_poll(now, Duration::from_millis(config.ping_interval)));
                        }))))
                    },
                    _ => Err(EngineError::InvalidPoll)
                }
            },
            EngineInput::Control(EngineIOClientCtrls::New(kind)) => {
                match &self.0 { 
                    EngineState::New { .. } => Ok(Some(EngineState::Connecting { start_time: now })),
                    _ => Err(EngineError::OpenFailed)
                }
            },
            EngineInput::Control(EngineIOClientCtrls::Close) => {
                Ok(Some(EngineState::Closed(EngineCloseReason::ClientClose)))
            }
        }
    }

    fn recv(&self, input:&EngineInput<EngineIOServerCtrls>, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>, EngineError> {
        todo!()
    }

    fn update(&mut self, next_state:EngineState, out_buffer:&mut VecDeque<IO>, config:&TransportConfig) -> &EngineState {
        match (&self.0, &next_state) {
            _ => {}
        }
        self.0 = next_state;
        return &self.0
    }

    fn next_deadline(&self, config:&TransportConfig) -> Option<Instant> {
        None
    }

}


