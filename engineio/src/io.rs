use std::collections::HashMap;
use std::task::Poll;
use futures_util::Future;
use futures_util::Stream;
use futures_util::TryFutureExt;
use tokio::select;
use tokio::time::Instant;
use crate::proto::MessageData;
use crate::proto::Payload;
use crate::proto::Sid;
use crate::proto::TransportConfig;
use crate::transport::TransportKind;
use crate::engine::EngineInput;
use crate::engine::Engine;
use crate::engine::EngineError;
use crate::engine::EngineSignal;
use crate::engine::EngineStateEntity;
use crate::engine::IO;
use crate::server::EngineIOServer;
use std::pin::Pin;
use tokio::sync::oneshot;
use tokio::sync::mpsc;


pub enum IOError<T> {
    SendError(T),
}

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
    tx: mpsc::Sender<(EngineInput, oneshot::Sender<Result<(),EngineError>>)>,
    rx: mpsc::Receiver<Payload>
}

pub enum SessionCloseReason {
    Unknown,
    TransportClose
}

impl  Session {
    pub async fn send(&self, payload:MessageData) -> Result<(),EngineError> {
        let (res_tx, res_rx) = oneshot::channel();
        let p = EngineInput::Data(Ok(Payload::Message(payload)));
        self.tx.send((p,res_tx)).map_err(|e|EngineError::Generic).await?;
        res_rx.await.unwrap_or_else(|_| Err(EngineError::Generic))
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
            Poll::Ready(Some(p)) => {
                match p {
                    Payload::Message(m) => Poll::Ready(Some(m)),
                    _ => Poll::Pending,
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
       return (1, None) 
    }
}

#[derive(Clone)]
pub struct MultiPlex {
    tx_new: mpsc::Sender<(Sid, oneshot::Sender<Session>)>,
    tx_data: mpsc::Sender<(Sid,EngineInput, oneshot::Sender<Result<(),EngineError>>)>,
    tx_listen: mpsc::Sender<(Sid, oneshot::Sender<Result<Vec<Payload>,EngineError>>)>
}

impl MultiPlex {
    pub async fn create(&self, sid:Sid) -> Result<Session, EngineError> {
        let res = oneshot::channel();
        self.tx_new.send((sid, res.0)).await;
        res.1.await.map_err(|_| EngineError::Generic).map(|s| Ok(s))?
    }

    pub async fn input(&self, sid:Sid, input:EngineInput) -> Result<(),EngineError> {
        let res = oneshot::channel();
        self.tx_data.send((sid, input,res.0 )).await.map_err(|_| EngineError::Generic)?;
        res.1.await.map_err(|_| EngineError::Generic)?
    }

    pub async fn listen(&self, sid:Sid) -> Result<Vec<Payload>,EngineError> {
        let res = oneshot::channel();
        self.tx_listen.send((sid, res.0)).await;
        res.1.await.map_err(|_| EngineError::Generic)?
    }
}

pub fn create_multiplex(config:TransportConfig) -> MultiPlex {
    let mut input_new = mpsc::channel::<(Sid, oneshot::Sender<Session>)>(1024);
    let mut input_data = mpsc::channel::<(Sid, EngineInput, oneshot::Sender<Result<(),EngineError>>)>(1024);
    let mut input_listen = mpsc::channel::<(Sid, oneshot::Sender<Result<Vec<Payload>,EngineError>>)>(1024);

    tokio::spawn(async move {
        let mut tx_map: HashMap<Sid,mpsc::Sender<(EngineInput,oneshot::Sender<Result<(),EngineError>>)>> = HashMap::new();
        let mut rx_map: HashMap<Sid,mpsc::Receiver<Payload>> = HashMap::new();
        let mut in_flight = tokio::task::JoinSet::<(Sid,mpsc::Receiver<Payload>)>::new();
        
        loop {
            let now = Instant::now();
            tokio::select! {
                Some(Ok((sid,rx))) = in_flight.join_next() => {
                    &rx_map.insert(sid, rx);
                }

                Some((sid,tx)) = input_new.1.recv() => {
                    let mut engine = Engine::new(EngineIOServer::new(sid, Instant::now().into()));
                    let _ = engine.recv(EngineInput::Control(EngineSignal::New(TransportKind::Poll)), now.into(), &config);
                    let session = create_session(engine, config, |tx,rx| {
                        let _ = &tx_map.insert(sid, tx);
                        let _ = &rx_map.insert(sid, rx);
                        return async move {
                            tokio::time::sleep(tokio::time::Duration::from_millis(1000000)).await;
                            return SessionCloseReason::Unknown
                        }
                    });
                    if let Err(session) = tx.send(session) {
                        // failed to send back session, clean up please
                       drop(session);
                       let _ = tx_map.remove(&sid);
                       let _ = rx_map.remove(&sid);
                    }
                },
                Some((sid,data,tx)) = input_data.1.recv() => {
                    let Some(t) = tx_map.get(&sid) else { 
                        let _ = tx.send(Err(EngineError::MissingSession));
                        continue
                    };
                    // Don't block main loop
                    let sender = (*t).clone();
                    tokio::spawn(async move {
                        // IF we fail to send to engine, assume its dead?
                        if let Err(e) = sender.send((data,tx)).await {
                            let (_,tx) = e.0;
                            let _ = tx.send(Err(EngineError::Generic));
                        }
                    });
                },
                Some((sid,tx)) = input_listen.1.recv() => {
                    if !rx_map.contains_key(&sid) {
                        tx.send(Err(EngineError::MissingSession));
                        continue 
                    }   
                    let (_,replace_rx) = mpsc::channel(1);
                    let mut rx = rx_map.remove(&sid).unwrap();
                    rx_map.insert(sid, replace_rx);
                    
                    in_flight.spawn(async move {
                        let mut res:Vec<Payload> = vec![];
                        let Some(first) = rx.recv().await else { 
                            tx.send(Err(EngineError::Generic));
                            return (sid,rx)
                        };
                        res.push(first);
                        loop {
                            tokio::select! {
                                _ = tokio::time::sleep(tokio::time::Duration::from_millis(10)) => break,
                                output = rx.recv() => {
                                    match output {
                                        None => break,
                                        Some(o) => res.push(o)
                                    }
                                }
                            }
                        }
                        tx.send(Ok(res));
                        (sid,rx)
                    });
                },
                else => break
            }
        }

    });

    return MultiPlex {
        tx_data: input_data.0,
        tx_listen: input_listen.0,
        tx_new: input_new.0
    }
    
}

pub fn create_session<T,Fut>(
    engine: Engine<T>, 
    config: TransportConfig,
    transport: impl FnOnce(mpsc::Sender<(EngineInput, oneshot::Sender<Result<(),EngineError>>)>,mpsc::Receiver<Payload>) -> Fut
) -> Session
where Fut: Future<Output = SessionCloseReason> + Send + 'static,
      T:EngineStateEntity + Send + Unpin + 'static,
{
    let (up_req, up_res) = engine_channel();
    let (down_req, down_res) = engine_channel();
    let t = transport(down_req.0, down_req.1);

    let handle = tokio::spawn(async move {
        let mut e = tokio::spawn(bind_engine(engine, config, down_res, up_res));
        let mut t = tokio::spawn(t);
        tokio::select! {
            t = &mut e => SessionCloseReason::Unknown,
            engine = &mut t=> SessionCloseReason::Unknown
        }
        // HERE we should drain the engine as needed ...
    });

    return Session { handle, tx:up_req.0, rx:up_req.1 };
}

pub fn create_session_local<T,Fut>(
    engine: Engine<T>, 
    config: TransportConfig,
    transport: impl FnOnce(mpsc::Sender<(EngineInput, oneshot::Sender<Result<(),EngineError>>)>,mpsc::Receiver<Payload>) -> Fut
) -> Session
where Fut: Future<Output = SessionCloseReason> + 'static,
      T:EngineStateEntity + Send + Unpin + 'static,
{
    let (up_req, up_res) = engine_channel();
    let (down_req, down_res) = engine_channel();
    let t = transport(down_req.0, down_req.1);

    let handle = tokio::task::spawn_local(async move {
        let mut e = tokio::spawn(bind_engine(engine, config, down_res, up_res));
        let mut t = tokio::task::spawn_local(t);
        tokio::select! {
            t = &mut e => SessionCloseReason::Unknown,
            engine = &mut t=> SessionCloseReason::Unknown
        }
        // HERE we should drain the engine as needed ...
    });

    return Session { handle, tx:up_req.0, rx:up_req.1 };
}

async fn bind_engine<T:EngineStateEntity>(
    mut engine:Engine<T>,
    mut config:TransportConfig,
    down_stream:EngineChannelRes<EngineInput,Payload,EngineError>,
    up_stream:EngineChannelRes<EngineInput,Payload,EngineError>) -> Engine<T>
{
    let (down_send, mut down_recv) = down_stream;
    let (up_send, mut up_recv) = up_stream;

    loop {
        let now = tokio::time::Instant::now();        
        let next = loop {
            match engine.poll(now.into(), &config) {
                Some(IO::Wait(d)) => {
                    break Ok(Some(d))
                }
                Some(IO::Send(p)) => {
                    if let Err(e) = down_send.send(p).await {
                        break Err(IOError::SendError(e.0))
                    }
                },
                Some(IO::Recv(p)) => {
                    // We don't mind if upstream has dropped
                    let _ = up_send.send(p).await;
                }
                None => {
                    break(Ok(None))
                }
            }
        };
        let Ok(Some(next_deadline)) = next else { break };
        select! {
            _ = tokio::time::sleep_until(next_deadline.into()) => {},
            Some((up,tx)) = up_recv.recv() => {
                tx.send(engine.send(up, now.into(), &config));
            }
            Some((down,tx)) = down_recv.recv() =>  {
                tx.send(engine.recv(down, now.into(), &config));
            }
        };
    };
    return engine

}









