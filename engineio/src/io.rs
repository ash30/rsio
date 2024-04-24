use std::task::Poll;
use futures_util::Stream;
use tokio::select;
use crate::engine::AsyncLocalTransport;
use crate::engine::AsyncTransport;
use crate::engine::Engine;
use crate::engine::EngineCloseReason;
use crate::engine::Input;
use crate::engine::Output;
use crate::proto::MessageData;
use crate::engine::EngineError;
use crate::proto::Payload;
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

async fn bind_engine<T:AsyncLocalTransport>(
    mut engine:impl Engine,
    mut transport:T,
    up_stream:EngineChannelRes<MessageData,MessageData,EngineError>) -> impl Engine{

    let (up_send, mut up_recv) = up_stream;
    loop {
        let now = tokio::time::Instant::now();        
        let next = loop {
            match engine.poll(now.into()) {
                Some(Output::Data(m)) => {
                    // TODO: ERROR?
                    let _ = up_send.send(m).await;
                },
                Some(Output::StateUpdate(s)) => { 
                    dbg!("Engine state change", &s);
                    let _ = engine.process_input(
                        Input::TransportUpdate(s, transport.engine_state_update(s).await), now.into()
                    );
                },
                Some(Output::Wait(t)) => break Some(t),
                None => {
                    break(None)
                }
            }
        };
        let Some(next_deadline) = next else { dbg!(); break engine};
        select! {
            _ = tokio::time::sleep_until(next_deadline.into()) => {},
            Some((up,tx)) = up_recv.recv() => {
                dbg!(&up);
                if engine.is_connected() {
                    // TODO: handle error 
                    let _ = transport.send(Payload::Message(up)).await;
                    tx.send(Ok(()));
                }
            }
            down = transport.recv() =>  {
                match down {
                    Ok(down) => {
                        // TODO: What about errors ?
                        dbg!(&down);
                        engine.process_input(Input::Recv(down), now.into());
                    }
                    Err(e) => {
                        //Transport Erroe
                        engine.close(EngineCloseReason::Error(EngineError::Transport(e)), now.into())
                    }
                }
            }
        };

    }
}








