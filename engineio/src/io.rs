use std::time::Duration;
use std::time::Instant;
use std::collections::HashMap;
use futures_util::Stream;
use tokio_stream::StreamExt;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::Receiver;
use crate::EngineError;
use crate::EngineCloseReason;
use crate::MessageData;
use crate::engine::{Sid, Payload, EngineInput, EngineIOClientCtrls, EngineIOServerCtrls, IO, EngineDataOutput} ;
use crate::server::EngineIOServer;
use crate::handler::ConnectionMessage;

pub trait AsyncConnectionService { 
    fn new_connection<S:Stream<Item=ConnectionMessage> + 'static + Send>(&self, connection:S, session:AsyncSessionServerHandle);
}

fn create_connection_stream(s:impl Stream<Item=Payload>) -> impl Stream<Item=Result<MessageData,EngineCloseReason>> {
    s.filter_map(|p| match p { 
        Payload::Message(d) => Some(Ok(d)),
        Payload::Close(r) => Some(Err(r)),
        _ => None,
    })
}

// =============================================

pub struct AsyncSessionServerHandle {
    sid:Sid,
    input_tx: Sender<(Sid, EngineInput<EngineIOServerCtrls>, AsyncInputSender)>,
}

impl AsyncSessionServerHandle {
   pub async fn send(&self, data:MessageData) -> Result<(),EngineError> {
        let (tx,rx) = tokio::sync::oneshot::channel::<AsyncInputResult>();
        let _ = self.input_tx.send(
            (self.sid, EngineInput::Data(Ok(Payload::Message(data))), tx)
        ).await;
        rx.await.unwrap_or(Err(EngineError::AlreadyClosed))?;
        Ok(())
   }
}

// =============================================

#[derive(Debug, Clone)]
pub struct AsyncSessionClientHandle {
    input_tx: Sender<(Sid, EngineInput<EngineIOClientCtrls>, AsyncInputSender)>,
}

impl AsyncSessionClientHandle {
    pub async fn input(&self, id:Sid, input:EngineInput<EngineIOClientCtrls>) -> Result<Option<impl Stream<Item =Payload>>,EngineError> {
        let (tx,rx) = tokio::sync::oneshot::channel::<AsyncInputResult>();
        let _ = self.input_tx.send((id,input, tx)).await;
        let res = rx.await.unwrap_or(Err(EngineError::AlreadyClosed))?;
        Ok(res.map(|r| tokio_stream::wrappers::ReceiverStream::new(r)))
    }

    pub async fn input_with_response_stream(&self, id:Sid, input:EngineInput<EngineIOClientCtrls>) -> Result<impl Stream<Item=Payload>,EngineError> {
        match self.input(id, input).await {
            Err(e) => Err(e),
            Ok(Some(s)) => Ok(s),
            Ok(None) => Err(EngineError::Generic)
        }
    }
}

// =============================================

type AsyncInputResult = Result<Option<Receiver<Payload>>,EngineError>;
type AsyncInputSender = tokio::sync::oneshot::Sender<AsyncInputResult>;

pub enum Either<A,B> {
    A(A),
    B(B)
}

type EngineInput2 = Either<EngineInput<EngineIOClientCtrls>, EngineInput<EngineIOServerCtrls>>;

pub fn create_async_io2<F>(client:F) -> AsyncSessionClientHandle 
where F:AsyncConnectionService + 'static + Send
{

    let (client_send_tx, mut client_send_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput<EngineIOClientCtrls>, AsyncInputSender)>(1024);
    let (server_send_tx, mut server_send_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput<EngineIOServerCtrls>, AsyncInputSender)>(1024);
    let mut workers: HashMap<Sid,Sender<(EngineInput2, AsyncInputSender)>> = HashMap::new();
    
    tokio::spawn( async move {
        loop {
            let (sid,input,res_tx) = tokio::select! {
                Some(client) = client_send_rx.recv() => (client.0, Either::A(client.1), client.2),
                Some(server) = server_send_rx.recv() => (server.0, Either::B(server.1), server.2),
            };

            let worker = match input {
                Either::A(EngineInput::Control(EngineIOClientCtrls::New(..))) => {
                    // Channel for IO_DISPATCHER to talk to worker
                    let (worker_recv_tx, worker_recv_rx) = tokio::sync::mpsc::channel::<(EngineInput2,AsyncInputSender)>(32);
                    workers.insert(sid, worker_recv_tx.clone());
                    // Channel for END_CLIENT to recv events 
                    let (server_recv_tx, mut server_recv_rx) = tokio::sync::mpsc::channel::<Payload>(10);
                    client.new_connection(
                        create_connection_stream(tokio_stream::wrappers::ReceiverStream::new(server_recv_rx)),
                        AsyncSessionServerHandle{ sid:sid.clone(), input_tx:server_send_tx.clone()  }
                    );

                    tokio::spawn(create_worker(sid, worker_recv_rx, server_recv_tx.clone()));
                    Some(worker_recv_tx)
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

    return AsyncSessionClientHandle { 
        input_tx: client_send_tx
    }
}

async fn create_worker(id:Sid, mut rx:tokio::sync::mpsc::Receiver<(EngineInput2,AsyncInputSender)>, tx: tokio::sync::mpsc::Sender<Payload>) {
    let mut engine = EngineIOServer::new(id);
    let mut send_buffer = vec![];
    let mut send_tx = None;
    let mut next_tick = Instant::now() + Duration::from_secs(10);
    loop {
        let now = Instant::now();
        let inputs = tokio::select! {
            input = rx.recv() => if let Some(i) = input { Some(i) } else { break },
            _  = tokio::time::sleep_until(next_tick.into()) => None
        };

        inputs.map(|(input,res_tx)| {
            let r = match input {
                Either::A(client) => engine.input_recv(client, now),
                Either::B(server) => engine.input_send(server, now),
            };
            match r {
                Ok(Some(IO::Open)) => {
                    let (tx,res_rx) = tokio::sync::mpsc::channel(32);
                    send_tx = Some(tx);
                    res_tx.send(Ok(Some(res_rx)));
                },
                Ok(Some(IO::Close)) => {send_tx = None;},
                Ok(None) => {res_tx.send(Ok(None));},
                Err(e) => { res_tx.send(Err(e));}
            }
        });
        
        let next_deadline = loop {
            match engine.poll_output(now) {
                Some(output) => {
                    match output {
                        EngineDataOutput::Recv(m) => { tx.send(m);},
                        EngineDataOutput::Send(p) => { 
                            if let Some(sender) = &send_tx {
                                sender.send(p).await;
                            }
                            else {
                                send_buffer.push(p);
                            }   
                        },
                        EngineDataOutput::Pending(d) => { break Some(now + d) }
                    }
                },
                None => break None// finished! 
            }
        };

        if let Some(nd) = next_deadline  {
            next_tick = nd;
        }
        else { break } // finished
    }
}







