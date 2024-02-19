use std::{collections::VecDeque, time::{Instant, Duration}};
use crate::{Sid, EngineOutput, EngineState, EngineInput, EngineIOClientCtrls, EngineError, ConnectedState, Transport, PollingState, Heartbeat, TransportConfig, EngineCloseReason, EngineIOServerCtrls, Payload, EngineKind, Either};

struct EngineIOClientState(EngineState);

impl EngineStateEntity for EngineIOClientState {
    type Receiver = EngineIOServerCtrls;
    type Sender = EngineIOClientCtrls;

    fn time(&self, now:Instant, config:&TransportConfig) -> (Option<Duration>, Option<Self>) {
        todo!()
    }

    fn send(&self, input:EngineInput<EngineIOClientCtrls>, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>,EngineError> {
        match input {
            EngineInput::Data(Err(e)) => Err(EngineError::UnknownPayload),

            EngineInput::Data(Ok(p)) => {
                Ok(None)
            },
            EngineInput::Control(EngineIOClientCtrls::Poll) => {
                match self.0 {
                    EngineState::Connected(s@ConnectedState(Transport::Polling(PollingState { active:None, count }),_,_)) => {
                        Ok(Some(EngineState::Connected(s.clone().update(|t,h| {
                            t.poll_state().map(|p| p.activate_poll(now, Duration::from_millis(config.ping_interval)));
                        }))))
                    },
                    _ => Err(EngineError::InvalidPoll)
                }
            },
            EngineInput::Control(EngineIOClientCtrls::New(config, kind)) => {
                match self.0 { 
                    EngineState::New { .. } => Ok(Some(EngineState::Connecting { start_time: now })),
                    _ => Err(EngineError::OpenFailed)
                }
            },
            EngineInput::Control(EngineIOClientCtrls::Close) => {
                Ok(Some(EngineState::Closed(EngineCloseReason::ClientClose)))
            }
        }
    }

    fn recv(&self, input:EngineInput<EngineIOServerCtrls>, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>, EngineError> {
        todo!()
    }

    fn update(&mut self, next_state:EngineState, out_buffer:&mut VecDeque<EngineOutput>) -> &EngineState {
        match (&self.0, &next_state) {
            _ => {}
        }
        self.0 = next_state;
        return &self.0
    }

    fn next_deadline(&self) -> Option<Instant> {
        None
    }

}

trait EngineStateEntity {
    type Sender;
    type Receiver;
    fn time(&self, now:Instant, config:&TransportConfig) -> Option<EngineState>;
    fn send(&self, input:EngineInput<Self::Sender>, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>, EngineError>;
    fn recv(&self, input:EngineInput<Self::Receiver>, now:Instant, config:&TransportConfig) -> Result<Option<EngineState>, EngineError>;
    fn update(&mut self, next_state:EngineState, out_buffer:&mut VecDeque<EngineOutput>) -> &EngineState;
    fn next_deadline(&self) -> Option<Instant>;
}

pub (crate) struct Engine<T> {
    pub session: Sid,
    output: VecDeque<EngineOutput>,
    state: T,
}

impl<T> Engine<T>
where T:EngineStateEntity {
    pub fn new(id:Sid, now:Instant, start_state:T) -> Self {
        return Self { 
            session: id,
            output: VecDeque ::new(),
            state: start_state,
        }
    }

    fn advance_time(&mut self, now:Instant, config:&TransportConfig) {
        while let Some(d) = self.state.next_deadline() {
            if now < d { break };
            if let Some(s) = self.state.time(now, config) {
                self.state.update(s, &mut self.output);
            }
        }
    }

    pub fn input(&mut self, i:Either<EngineInput<T::Sender>,EngineInput<T::Receiver>>, now:Instant, config:&TransportConfig) -> Result<(),EngineError> {
        self.advance_time(now, config);
        let next_state = match i {
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
                    let e = self.state.update(s, &mut self.output).has_error();
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
            self.state.next_deadline().map(|d| EngineOutput::Pending(d))
        })

    }

}

