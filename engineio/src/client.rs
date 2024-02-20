use std::{collections::VecDeque, time::{Instant, Duration}};
use crate::{Sid, EngineOutput, EngineState, EngineInput, EngineIOClientCtrls, EngineError, ConnectedState, Transport, PollingState, TransportConfig, EngineCloseReason, EngineIOServerCtrls, EngineStateEntity};

pub (crate) struct EngineIOClient(EngineState,Sid);

impl EngineIOClient {
    pub fn new(sid:Sid,now:Instant) -> Self {
       return Self(EngineState::New { start_time: now }, sid)
    }
}

impl EngineStateEntity for EngineIOClient {
    type Receiver = EngineIOServerCtrls;
    type Sender = EngineIOClientCtrls;

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
                    EngineState::Connected(s@ConnectedState(Transport::Polling(PollingState { active:None, count }),_)) => {
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

    fn update(&mut self, next_state:EngineState, out_buffer:&mut VecDeque<EngineOutput>, config:&TransportConfig) -> &EngineState {
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


