use std::task::Poll;
use std::time::Instant;
use futures_util::Stream;
use crate::engine::AsyncLocalTransport;
use crate::engine::AsyncTransport;
use crate::engine::Engine;
use crate::engine::EngineCloseReason;
use crate::engine::Input;
use crate::engine::Output;
use crate::proto::MessageData;
use crate::engine::EngineError;
use crate::proto::Payload;
use crate::transport::TransportError;
use std::pin::Pin;
use tokio::sync::oneshot;
use tokio::sync::mpsc;

type EngineChannelReq<T,R,E> = (mpsc::Sender<(T, oneshot::Sender<Result<(),E>>)>, mpsc::Receiver<R>);
type EngineChannelRes<T,R,E> = (mpsc::Sender<R>, mpsc::Receiver<(T, oneshot::Sender<Result<(),E>>)>);
type EngineChannelPair<T,R,E> = (EngineChannelReq<T,R,E>, EngineChannelRes<T,R,E>);

fn engine_channel<T,R,E>() -> EngineChannelPair<T,R,E> {
    let (req_tx, req_rx) = tokio::sync::mpsc::channel(10);
    let (res_tx, res_rx) = tokio::sync::mpsc::channel(10);
    return (
        (req_tx, res_rx),
        (res_tx, req_rx)
    )
}

#[pin_project::pin_project]
pub struct Session { 
    #[pin]
    handle: tokio::task::JoinHandle<SessionCloseReason>,
    tx: mpsc::Sender<(MessageData, oneshot::Sender<Result<(),EngineError>>)>,
    rx: mpsc::Receiver<MessageData>
}

pub enum SessionCloseReason {
    Unknown,
    TransportClose
}

impl  Session {
    pub async fn send(&self, payload:MessageData) -> Result<(),EngineError> {
        let (res_tx, res_rx) = oneshot::channel();
        self.tx.send((payload,res_tx)).await.map_err(|e|EngineError::Generic)?;
        res_rx.await.unwrap_or_else(|_| Err(EngineError::Generic))
    }

    pub async fn send_binary(&self, data:Vec<u8>) -> Result<(),EngineError> {
        self.send(MessageData::Binary(data)).await
    }

    pub async fn send_text(&self, data:impl Into<String>) -> Result<(),EngineError> {
        // TODO: better conversion?
        self.send(MessageData::String(data.into().as_bytes().to_vec())).await
    }
}

impl Stream for Session {
    type Item = MessageData;
    
    fn poll_next(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        match this.rx.poll_recv(cx){
            // TODO: WORK OUT PINNING API PLEASE 
            Poll::Ready(None) => {
                Poll::Ready(None)
                //match this.handle.as_mut().poll(cx) {
                //    Poll::Pending => Poll::Pending,
                //    Poll::Ready(Ok(s)) => Poll::Ready(Some(Err(s))),
                //    Poll::Ready(Err(e)) => Poll::Ready(Some(Err(SessionCloseReason::Unknown)))
                //}
            },
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(m)) => Poll::Ready(Some(m))
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
       return (1, None) 
    }
}

pub fn create_session<T>(
    engine: impl Engine + Send + 'static, 
    transport: T
) -> Session
where
      T:AsyncTransport + Send + Unpin + 'static,
{
    let (up_req, up_res) = engine_channel();
    let handle = tokio::task::spawn(async move {
        bind_engine(engine, transport, up_res).await;
        SessionCloseReason::Unknown
    });

    return Session { handle, tx:up_req.0, rx:up_req.1 };
}
pub fn create_session_local<T>(
    engine: impl Engine + 'static, 
    transport: T
) -> Session
where
      T:AsyncLocalTransport + Unpin + 'static,
{
    let (up_req, up_res) = engine_channel();
    let handle = tokio::task::spawn_local(async move {
        bind_engine(engine, transport, up_res).await;
        SessionCloseReason::Unknown
    });

    return Session { handle, tx:up_req.0, rx:up_req.1 };
}

async fn bind_engine(
    mut engine: impl Engine, 
    mut transport: impl AsyncLocalTransport, 
    up_stream:EngineChannelRes<MessageData,MessageData,EngineError>) -> impl Engine 
{
    let (up_send, mut up_recv) = up_stream;

    // drive engine state terminal state 
    while engine.is_closed() == false {
        if up_send.is_closed() { break }

        match engine.poll(Instant::now()) {
            Some(Output::StateUpdate(s)) => {
                // On state update, drive transport towards state
                let deadline = s.deadline();
                tokio::select! {
                   res = transport.engine_state_update(s) => {
                        engine.process(Input::TransportUpdate(s,res), Instant::now());
                   },
                    _ = tokio::time::sleep_until(deadline.unwrap().into()), if deadline.is_some() => {},
                    _ = up_send.closed() => {}
                }
            },
            Some(Output::Wait(until)) => {
                // Implicitly, we wait once we're connected..
                // TODO: can we make this api nicer?
                tokio::select! {
                    Some((send,tx)) = up_recv.recv() => {
                        if engine.is_connected() {
                            let _ = transport.send(Payload::Message(send)).await;
                            tx.send(Ok(()));
                        }
                        else {
                            tx.send(Err(EngineError::Generic));
                        }
                    },
                    recv = transport.recv() => {
                        let now = Instant::now();
                        match recv {
                            Ok(r) => {
                                // TODO: What about errors ?
                                if let Ok(()) = engine.process(Input::Recv(&r), now.into()) {
                                    if let Payload::Message(m) = r {
                                        let _ = up_send.send(m).await;
                                    }
                                }
                            }
                            Err(e) => {
                                engine.close(now.into(), Some(EngineCloseReason::Error(EngineError::Transport(e))))
                            }
                        }
                    },
                    // check the engine after current deadline
                    _ = tokio::time::sleep_until(until.into()) => {},
                    // exit if session owner drops 
                    _ = up_send.closed() => {}
                };
            }
            None => break
        }
    }
    engine

}






