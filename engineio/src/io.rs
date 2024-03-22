use std::collections::HashMap;
use std::fmt::Debug;
use std::task::Poll;
use futures_util::Future;
use futures_util::Stream;
use tokio::select;
use tokio::time::Instant;
use crate::engine::EngineIOClientCtrls;
use crate::proto::PayloadDecodeError;
use crate::proto::Payload;
use crate::proto::Sid;
use crate::proto::TransportConfig;
use crate::engine::EngineInput;
use crate::engine::Engine;
use crate::engine::EngineError;
use crate::engine::EngineStateEntity;
use crate::engine::IO;
use crate::server::EngineIOServer;
use std::pin::Pin;
use tokio::sync::oneshot;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum Either<A,B> {
    A(A),
    B(B)
}

pub enum IOError<T> {
    InternalError,
    SendError(T),
    TransportError(T)
}
pub enum IOCloseReason {}
type EngineChannelReq<T,R,E> = (mpsc::Sender<(T, oneshot::Sender<Result<(),E>>)>, mpsc::Receiver<R>);
type EngineChannelRes<T,R,E> = (mpsc::Sender<R>, mpsc::Receiver<(T, oneshot::Sender<Result<(),E>>)>);
type EngineChannelPair<T,R,E> = (EngineChannelReq<T,R,E>, EngineChannelRes<T,R,E>);

fn engine_channel<T,R,E>() -> EngineChannelPair<T,R,E> {
    let (req_tx, req_rx) = tokio::sync::mpsc::channel(1);
    let (res_tx, res_rx) = tokio::sync::mpsc::channel(1);
    return (
        (req_tx, res_rx),
        (res_tx, req_rx)
    )
}


#[pin_project::pin_project]
pub struct Session<T> { 
    #[pin]
    handle: tokio::task::JoinHandle<SessionCloseReason>,
    tx: mpsc::Sender<(EngineInput<T>, oneshot::Sender<Result<(),EngineError>>)>,
    rx: mpsc::Receiver<Payload>
}

pub enum SessionCloseReason {
    Unknown
}

impl <T> Session<T> {

    async fn send<P:TryInto<Payload, Error=PayloadDecodeError>>(&self, payload:P) -> Result<(),EngineError> {
        let (res_tx, res_rx) = oneshot::channel();
        let p = EngineInput::Data(payload.try_into());
        self.tx.send((p,res_tx));
        res_rx.await.unwrap_or_else(|_| Err(EngineError::Generic))
    }
}

impl <T> Stream for Session<T> {
    type Item = Payload;
    
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
            Poll::Ready(Some(p)) => Poll::Ready(Some(p))
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
       return (1, None) 
    }
}

#[derive(Clone)]
pub struct MultiPlex {
    tx_new: mpsc::Sender<(Sid, oneshot::Sender<Session<EngineIOClientCtrls>>)>,
    tx_input: mpsc::Sender<(Sid,EngineInput<EngineIOClientCtrls>, oneshot::Sender<Result<(),EngineError>>)>,
    tx_listen: mpsc::Sender<(Sid, oneshot::Sender<Result<Vec<Payload>,EngineError>>)>
}

impl MultiPlex {
    pub async fn create(&self, sid:Sid) -> Result<Session<EngineIOClientCtrls>, EngineError> {
        let res = oneshot::channel();
        self.tx_new.send((sid, res.0));
        res.1.await.map_err(|_| EngineError::Generic).map(|s| Ok(s))?
    }

    pub async fn input(&self, sid:Sid, input:EngineInput<EngineIOClientCtrls>) -> Result<(),EngineError> {
        let res = oneshot::channel();
        self.tx_input.send((sid, input,res.0 )).await.map_err(|_| EngineError::Generic)?;
        res.1.await.map_err(|_| EngineError::Generic)?
    }

    pub async fn listen(&self, sid:Sid) -> Result<Vec<Payload>,EngineError> {
        let res = oneshot::channel();
        self.tx_listen.send((sid, res.0));
        res.1.await.map_err(|_| EngineError::Generic)?
    }
}

pub fn create_multiplex() -> MultiPlex {
    let tx_map: HashMap<Sid,mpsc::Sender<(EngineInput<EngineIOClientCtrls>,oneshot::Sender<Result<(),EngineError>>)>> = HashMap::new();
    let rx_map: HashMap<Sid,mpsc::Receiver<Payload>>;
    let input_new = mpsc::channel::<(Sid, oneshot::Sender<Session<EngineIOClientCtrls>>)>(0);
    let input_data = mpsc::channel::<(Sid, EngineInput<EngineIOClientCtrls>, oneshot::Sender<Result<(),EngineError>>)>(0);
    let input_listen = mpsc::channel::<(Sid, oneshot::Sender<Result<Vec<Payload>,EngineError>>)>(0);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some((sid,tx)) = input_new.1.recv() => {
                    let engine = Engine::new(EngineIOServer::new(sid, Instant::now().into()));
                    //let (close_tx, close_rx) = oneshot::channel();
                    let session = create_session(engine, |tx,rx| {
                        tx_map.insert(sid, tx);
                        rx_map.insert(sid, rx);
                        // TODO: return proper close 
                        return async move {
                            return SessionCloseReason::Unknown
                        }
                    });
                    //tx.send(session);
                },
                Some((sid,data,tx)) = input_data.1.recv() => {
                    let Some(t) = tx_map.get(&sid) else { 
                        tx.send(Err(EngineError::Generic));
                        continue
                    };
                    // Don't block main loop
                    tokio::spawn(t.send((data,tx)));
                },
                Some((sid,tx)) = input_listen.1.recv() => {
                    let Some(r) = rx_map.get(&sid) else { 
                        tx.send(Err(EngineError::Generic));
                        continue
                    };
                    tokio::spawn(async move {
                        let mut res:Vec<Payload> = vec![];
                        let Some(first) = r.recv().await else { 
                            tx.send(Err(EngineError::Generic));
                            return
                        };
                        res.push(first);
                        loop {
                            tokio::select! {
                                _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {},
                                output = r.recv() => {
                                    match output {
                                        None => break,
                                        Some(o) => res.push(o)
                                    }
                                }
                            }
                        }
                        tx.send(Ok(res));
                    });
                },
                else => break
            }
        }

    });

    return MultiPlex {
        tx_input: input_data.0,
        tx_listen: input_listen.0,
        tx_new: input_new.0
    }
    
}




pub fn create_session<T,Fut>(
    engine: Engine<T>, 
    transport: impl FnOnce(mpsc::Sender<(EngineInput<T::Receive>, oneshot::Sender<Result<(),EngineError>>)>,mpsc::Receiver<Payload>) -> Fut
) -> Session<T::Send>
where Fut: Future<Output = SessionCloseReason> + Send,
      T:EngineStateEntity + Send + Unpin,
      T::Receive: Send,
      T::Send: Send
{
    let (up_req, up_res) = engine_channel();
    let (down_req, down_res) = engine_channel();
    let t = transport(down_req.0, down_req.1);

    let handle = tokio::spawn(async move {
        let e = tokio::spawn(bind_engine(engine, down_res, up_res));
        let t = tokio::spawn(t);
        tokio::select! {
            t = &mut t => SessionCloseReason::Unknown,
            engine = &mut t=> SessionCloseReason::Unknown
        }
        // HERE we should drain the engine as needed ...
    });

    return Session { handle, tx:up_req.0, rx:up_req.1 };
}

// TODO: Can we refactor this ??
pub fn create_session_local<T,Fut>(
    engine: Engine<T>, 
    transport: impl FnOnce(mpsc::Sender<(EngineInput<T::Receive>, oneshot::Sender<Result<(),EngineError>>)>,mpsc::Receiver<Payload>) -> Fut
) -> Session<T::Send>
where Fut: Future<Output = SessionCloseReason>,
      T:EngineStateEntity + Send + Unpin,
      T::Receive: Send,
      T::Send: Send
{
    let (up_req, up_res) = engine_channel();
    let (down_req, down_res) = engine_channel();
    let t = transport(down_req.0, down_req.1);

    let handle = tokio::task::spawn_local(async move {
        let e = tokio::spawn(bind_engine(engine, down_res, up_res));
        let t = tokio::task::spawn_local(t);
        tokio::select! {
            t = &mut t => SessionCloseReason::Unknown,
            engine = &mut t=> SessionCloseReason::Unknown
        }
        // HERE we should drain the engine as needed ...
    });

    return Session { handle, tx:up_req.0, rx:up_req.1 };
}

async fn bind_engine<T:EngineStateEntity>(
    mut engine:Engine<T>,
    down_stream:EngineChannelRes<EngineInput<T::Receive>,Payload,EngineError>,
    up_stream:EngineChannelRes<EngineInput<T::Send>,Payload,EngineError>) -> Engine<T>
{
    let (down_send, mut down_recv) = down_stream;
    let (up_send, mut up_recv) = up_stream;
    let config = TransportConfig::default();

    loop {
        let config = TransportConfig::default();
        let now = tokio::time::Instant::now();        

        let next = loop {
            match engine.poll(now.into(), &config) {
                Some(IO::Wait(d)) => {
                    break Ok(Some(d))
                }
                Some(IO::Send(p)) => {
                    if let Err(e) = down_send.send(Payload::Ping).await {
                        break Err(IOError::SendError(e.0))
                    }
                },
                Some(IO::Recv(p)) => {
                    // We don't mind if upstream has dropped
                    let _ = up_send.send(p).await;
                }
                Some(_) => {
                    // we ingore others for now ...
                }
                None => {
                    break(Ok(None))
                }
            }
        };
        let Ok(Some(next_deadline)) = next else { break };
        let input = select! {
            _ = tokio::time::sleep_until(next_deadline.into()) => None,
            Some((up,tx)) = up_recv.recv() => Some((Either::A(up), tx)),
            Some((down,tx)) = down_recv.recv() => Some((Either::B(down), tx)),
        };
        if let Some((p,res_tx)) = input {
           res_tx.send(engine.input(p,now.into(),&config)); 
        }
    };
    return engine

}









