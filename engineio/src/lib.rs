mod proto;

use futures_util::Stream;
use std::future::Future;
use tokio::time::Sleep;
use std::{time::{Instant, Duration}, error::Error};
use pin_project::pin_project;
use std::pin::pin;
use std::task::Poll;

#[derive(Copy, Clone, PartialEq)]
pub enum HeartbeatState {
    Alive,
    Unknown,
    Dead,
}

pub struct Heartbeat {
    ping_interval: Duration,
    ping_timeout: Duration,
    last_update:Instant,
    state: HeartbeatState,
}

enum Output<T> {
    Pending(Instant),
    Output(T)
}

impl Heartbeat {
    pub fn next_new(&mut self, now:Instant) -> Output<Option<HeartbeatState>> {
        todo!();
    }

    pub fn touch(&mut self, now:Instant) {
        match self.state {
            HeartbeatState::Dead => {},
            _ => {
                self.last_update = now;
                self.state = HeartbeatState::Alive;
            }
        }
    }

    pub fn deadline(&self) -> Instant {
            self.last_update + self.ping_interval + self.ping_timeout
    }
}

struct EngineError {}

trait PayloadSender {
    async fn send(&mut self, p:proto::Payload) -> Result<(),EngineError> ;
}

#[pin_project]
struct EngineReceiver<S,F:Fn() -> FF,FF> {
    #[pin]
    transport: S,
    heartbeat: Heartbeat,
    #[pin]
    timer: Sleep,
    pinger: F,
    #[pin]
    current: Option<FF>
}

impl <S,F,FF> Stream for EngineReceiver<S,F,FF> 
where
    S:Stream<Item=proto::Payload>,
    FF: Future<Output=Result<(),EngineError>>,
    F: Fn() -> FF
{
    type Item = proto::Payload;
    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
        let mut this = self.project();
        let now = Instant::now();
        if now > this.heartbeat.deadline() {
            return Poll::Ready(None)
        }

        if let Some(f) = this.current.as_mut().as_pin_mut() {
            if let Poll::Ready(_) = f.poll(cx) {
                this.current.set(None);
            }
            return Poll::Pending
        }

        if let Poll::Ready(v) = this.transport.poll_next(cx) {
            this.heartbeat.touch(now);
            this.timer.reset(this.heartbeat.deadline().into());
            return Poll::Ready(v);
        };

        // NOTE:must set initial timer in new!!
        match this.timer.as_mut().poll(cx) {
            Poll::Ready(()) => {
                let next_deadline = loop {
                    match this.heartbeat.next_new(now) {
                        Output::Output(Some(s)) => {
                            match s {
                                HeartbeatState::Unknown => {
                                    // DO PING!
                                    let f = (this.pinger)();
                                    this.current.set(Some(f));
                                }
                                _ => {}
                            }
                        }
                        Output::Output(None) => break None,
                        Output::Pending(t) => break Some(t)
                    }
                };
                if let Some(t) = next_deadline {
                    this.timer.reset(t.into());
                    Poll::Pending
                }
                else {
                    // We don't reset timer so its NOT safe to poll again...
                    // This needs to align with initial guard statement
                    // eg. now > this.heartbeat.deadline() 
                    Poll::Ready(None)
                }
            },
            _ => Poll::Pending
        }
    }
}


