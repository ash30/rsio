use std::collections::VecDeque;
use std::time::Duration;
use std::time::Instant;
use std::collections::HashMap;
use futures_util::Stream;
use tokio_stream::StreamExt;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::Receiver;
use crate::EngineError;
use crate::EngineCloseReason;
use crate::EngineInput;
use crate::EngineOutput;
use crate::MessageData;
use crate::client::EngineIOClientCtrls;
use crate::client::EngineIOServerCtrls;
use crate::client::IO;
use crate::engine::{Sid, Payload, Engine, Participant} ;
use crate::handler::ConnectionMessage;

pub trait AsyncConnectionService { 
    fn new_connection<S:Stream<Item=ConnectionMessage> + 'static + Send>(&self, connection:S, session:AsyncSession);
}

fn create_connection_stream(s:impl Stream<Item=Payload>) -> impl Stream<Item=Result<MessageData,EngineCloseReason>> {
    s.filter_map(|p| match p { 
        Payload::Message(d) => Some(Ok(d)),
        Payload::Close(r) => Some(Err(r)),
        _ => None,
    })
}

// =============================================

pub struct AsyncSession {
    sid:Sid,
    handle: AsyncIOHandle
}

impl AsyncSession {
    pub fn new(sid:Sid, handle:AsyncIOHandle) -> Self {
        Self {
            sid, handle
        }
    }

   pub async fn send(&self, data:MessageData) {
       // TODO: Listen for SEND errors ?
       self.handle.input(
           self.sid, 
           EngineInput::Data(Participant::Server, Ok(Payload::Message(data))),
        ).await;
   }
}

// =============================================

type AsyncInputResult = Result<Option<Receiver<Payload>>,EngineError>;
type AsyncInputSender = tokio::sync::oneshot::Sender<AsyncInputResult>;

pub enum Either<A,B> {
    A(A),
    B(B)
}

type EngineInput2 = Either<crate::client::EngineInput<EngineIOClientCtrls>, crate::client::EngineInput<EngineIOServerCtrls>>;

pub fn create_async_io2<F>(client:F) -> AsyncIOHandle 
where F:AsyncConnectionService + 'static + Send
{

    let (client_input_tx, mut client_input_rx) = tokio::sync::mpsc::channel::<(Sid, crate::client::EngineInput<EngineIOClientCtrls>, AsyncInputSender)>(1024);
    let (server_input_tx, mut server_input_rx) = tokio::sync::mpsc::channel::<(Sid, crate::client::EngineInput<EngineIOServerCtrls>, AsyncInputSender)>(1024);
    let mut workers: HashMap<Sid,Sender<(EngineInput2, AsyncInputSender)>> = HashMap::new();
    
    tokio::spawn( async move {
        loop {
            let (sid,input,res_tx) = tokio::select! {
                Some(client) = client_input_rx.recv() => (client.0, Either::A(client.1), client.2),
                Some(server) = server_input_rx.recv() => (server.0, Either::B(server.1), server.2),
            };

            let worker = match input {
                Either::A(crate::client::EngineInput::Control(EngineIOClientCtrls::New(..))) => {
                    // Channel for IO_DISPATCHER to talk to worker
                    let (tx,rx) = tokio::sync::mpsc::channel::<(EngineInput2,AsyncInputSender)>(32);
                    workers.insert(sid, tx.clone());
                    // Channel for END_CLIENT to recv events 
                    let (server_output_tx, mut server_output_rx) = tokio::sync::mpsc::channel::<Payload>(10);
                    client.new_connection(
                        create_connection_stream(tokio_stream::wrappers::ReceiverStream::new(server_output_rx)),
                        AsyncSession::new(sid.clone(), AsyncIOHandle { input_tx: server_input_tx.clone() } )
                    );

                    tokio::spawn(async move {
                        let engine = crate::client::EngineIOServer::new(sid);
                        let mut send_buffer = VecDeque::new();
                        let mut server_send_tx = None; 
                        let mut next_tick = Instant::now() + Duration::from_secs(10);
                        loop {
                            let now = Instant::now();
                            let input = tokio::select! {
                                input = rx.recv() => if let Some(i) = input { input } else { break },
                                time  = tokio::time::sleep_until(next_tick.into()) => None
                            };

                            let res = input.map(|(input,res_tx)| {
                                match input {
                                    Either::A(client) => engine.input_recv(client, now),
                                    Either::B(server) => engine.input_send(server, now),
                                }.map_err(|e| (res_tx,e))
                            });

                            if let Some(Err((tx,e))) = res {
                                tx.send(Err(e));
                                continue;
                            }

                            let mut res_stream = None;
                            loop {
                                match engine.poll_output(now) {
                                    Either::A(io) => {
                                        match io {
                                            IO::Open => {
                                                let (tx,rx) = tokio::sync::mpsc::channel(10);
                                                res_stream = rx.into();
                                                server_send_tx = tx.into();
                                            },
                                            IO::Close => { server_send_tx = None; },
                                            IO::Send(p) => { send_buffer.push_back(p);},
                                            IO::Recv(p) => { server_output_tx.send(p);},
                                            IO::Flush => {}
                                        }
                                        send_buffer.push_back(io)
                                    },
                                    Either::B(pending) => { next_tick =  now + pending.0; break } 
                                }
                            };

                            if let Some((_, res_tx)) = input { res_tx.send( res_stream)};
                        }
                    });
            
                    Some(tx)
                },
                _ => {
                    workers.get(&sid).map(|t| t.to_owned())
                }
            };

            if let Some(w) = worker {
               if let Err(e) = w.send_timeout((input,res_tx), Duration::from_secs(1)).await {
                    res_tx.send(Result::Err(EngineError::Generic));
               }
            }
            else {
                res_tx.send(Result::Err(EngineError::MissingSession));
            }
        }
    });
}

pub fn create_async_io<F>(client:F) -> AsyncIOHandle 
where F:AsyncConnectionService + 'static + Send
{
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncInputSender>)>(1024);
    let (time_tx, mut time_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncInputSender>)>(1024);

    let mut engines: HashMap<Sid,Engine> = HashMap::new();
    let mut server_recv: HashMap<Sid, Sender<Payload>> = HashMap::new();
    let mut client_recv: HashMap<Sid, Sender<Payload>> = HashMap::new();

    let input_tx_server = input_tx.clone();
    tokio::spawn( async move {
        loop {
            let (sid, input, tx)  = tokio::select! {
                Some(v1) = input_rx.recv() => v1,
                Some(v3) = time_rx.recv() => v3
            };

            let engine = match &input  {
                EngineInput::New(..) => { 
                    let (tx,rx) = tokio::sync::mpsc::channel::<Payload>(10);
                    server_recv.insert(sid.clone(), tx);
                    client.new_connection(
                        create_connection_stream(tokio_stream::wrappers::ReceiverStream::new(rx)),
                        AsyncSession::new(sid.clone(), AsyncIOHandle { input_tx: input_tx_server.clone() } )
                    );
                    Some(engines.entry(sid).or_insert(Engine::new(sid)))
                },
                _ => engines.get_mut(&sid)
            };

            let output = engine.ok_or(EngineError::OpenFailed)
                .and_then(|e| e.consume(input, Instant::now()).and(Ok(e)))
                .and_then(|e| { let mut b = vec![]; while let Some(p) = e.poll_output() { b.push(p); }; Ok(b) });

            let response = match output {
                Err(e) => {
                    Err(e)
                },
                Ok(output) => {
                    let mut res = None;
                    for o in output {
                        match o {
                            EngineOutput::Data(participant, data) => {
                                let map = if let Participant::Client = participant { &mut server_recv } else { &mut client_recv };
                                let sender = map.get(&sid);
                                dbg!(sender.is_some());
                                if let Some(sender) = sender {
                                    dbg!(sender.send(data).await);
                                };
                            },
                            EngineOutput::SetIO(participant, true ) => {
                                let map = if let Participant::Server = participant { &mut server_recv } else { &mut client_recv };
                                let (tx,rx) = tokio::sync::mpsc::channel::<Payload>(10);
                                map.insert(sid, tx);
                                res = Some(rx);
                            },
                            EngineOutput::SetIO(participant, false) => {
                                let map = if let Participant::Server = participant { &mut server_recv } else { &mut client_recv };
                                map.remove(&sid);
                            },
                            EngineOutput::Tick { length } => {
                                let tx = time_tx.clone();
                                tokio::spawn(async move { 
                                    tokio::time::sleep(length).await; 
                                    tx.send((sid, EngineInput::Tock, None)).await
                                });
                            }
                        }
                    }
                    Ok(res)
                }
            };

            if let Some(tx) = tx { 
                dbg!(&response);
                tx.send(response);
            }

        }
    });

    return AsyncIOHandle {
        input_tx,
    }
}

#[derive(Debug, Clone)]
pub struct AsyncIOHandle {
    input_tx: Sender<(Sid, EngineInput, Option<AsyncInputSender>)>,
}

impl AsyncIOHandle {
    pub async fn input(&self, id:Sid, input:EngineInput) -> Result<Option<impl Stream<Item =Payload>>,EngineError> {
        let (tx,rx) = tokio::sync::oneshot::channel::<AsyncInputResult>();
        let _ = self.input_tx.send((id,input, Some(tx))).await;
        let res = rx.await.unwrap_or(Err(EngineError::AlreadyClosed))?;
        Ok(res.map(|r| tokio_stream::wrappers::ReceiverStream::new(r)))
    }

    pub async fn input_with_response_stream(&self, id:Sid, input:EngineInput) -> Result<impl Stream<Item=Payload>,EngineError> {
        match self.input(id, input).await {
            Err(e) => Err(e),
            Ok(Some(s)) => Ok(s),
            Ok(None) => Err(EngineError::Generic)
        }
    }
}






