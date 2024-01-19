use std::time::Instant;
use std::collections::HashMap;
use futures_util::Stream;

use futures_util::StreamExt;
use tokio::sync::mpsc::Sender;
use crate::EngineCloseReason;
use crate::EngineInputError;
use crate::EngineInput;
use crate::EngineOutput;
use crate::EngineError;
use crate::engine::{Sid, Payload, Engine, Participant} ;

type AsyncIOSender = Sender<Result<Payload,EngineInputError>>;

pub fn async_session_io_create() -> AsyncSessionIOHandle {
    let (client_sent_tx, mut client_sent_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncIOSender>)>(1024);
    let (server_sent_tx, mut server_sent_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncIOSender>)>(1024);
    let (time_tx, mut time_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncIOSender>)>(1024);

    let mut engines: HashMap<Sid,Engine> = HashMap::new();
    let mut server_recv: HashMap<Sid, AsyncIOSender> = HashMap::new();
    let mut client_recv: HashMap<Sid, AsyncIOSender> = HashMap::new();

    tokio::spawn( async move {
        loop {
            let (sid, input, tx)  = tokio::select! {
                Some(v1) = client_sent_rx.recv() => v1,
                Some(v2) = server_sent_rx.recv() => v2,
                Some(v3) = time_rx.recv() => v3
            };

            dbg!(&input);

            let engine = match &input  {
                EngineInput::New(..) => Some(engines.entry(sid).or_insert(Engine::new(sid))),
                _ => engines.get_mut(&sid)
            };

            // Setup listeners 
            match (&input,&tx) {
                (EngineInput::Listen(Participant::Server),Some(sender)) => { server_recv.insert(sid, sender.clone());},
                (EngineInput::Listen(Participant::Client),Some(sender)) => { client_recv.insert(sid, sender.clone());}
                _ => {}
            };

            let output = engine.ok_or(EngineInputError::AlreadyClosed)
                .and_then(|e| e.consume(input, Instant::now()).and(Ok(e)))
                .and_then(|e| { let mut b = vec![]; while let Some(p) = e.poll_output() { b.push(p); }; Ok(b) });

            match output {
                Err(e) => {
                    if let Some(t) = tx {
                        t.send(Err(e)).await;
                    }
                },
                Ok(output) => {
                    for o in output {
                        match o {
                            EngineOutput::Data(participant, data) => {
                                let sender = if let Participant::Client = participant {server_recv.get(&sid)} else { client_recv.get(&sid) };
                                if let Some(sender) = sender {
                                    sender.send(Ok(data)).await;
                                };
                            },
                            EngineOutput::Reset => {

                            },
                            EngineOutput::Tick { length } => {
                                let tx = time_tx.clone();
                                tokio::spawn(async move { 
                                    tokio::time::sleep(length).await; 
                                    tx.send((sid, EngineInput::Tock, None)).await
                                });
                            }
                        };
                    };
                    drop(tx);
                }
            };

        }
    });

    return AsyncSessionIOHandle {
        client_send_tx:client_sent_tx,
        server_send_tx:server_sent_tx,
    }
}

#[derive(Debug, Clone)]
pub struct AsyncSessionIOHandle {
    client_send_tx: Sender<(Sid, EngineInput, Option<Sender<Result<Payload,EngineInputError>>>)>,
    server_send_tx: Sender<(Sid, EngineInput, Option<Sender<Result<Payload, EngineInputError>>>)>,
}

impl AsyncSessionIOHandle {

    pub async fn input(&self, id:Sid, input:EngineInput) -> impl Stream<Item=Result<Payload,EngineInputError>> {
        let (tx,rx) = tokio::sync::mpsc::channel::<Result<Payload,EngineInputError>>(10);
        let _ = self.client_send_tx.send((id,input, Some(tx))).await;
        return tokio_stream::wrappers::ReceiverStream::new(rx);
    }
}

pub struct AsyncSessionIOSender {
    sid:Sid,
    handle: AsyncSessionIOHandle
}

impl AsyncSessionIOSender {
    pub fn new(sid:Sid, handle:AsyncSessionIOHandle) -> Self {
        Self {
            sid, handle
        }
    }

   pub async fn send(&self, payload:Payload) {
       // TODO: Listen for SEND errors ?
       self.handle.input(self.sid, EngineInput::Data(Participant::Server, payload)).await;
   }
}


pub async fn session_collect(s:impl Stream<Item=Result<Payload,EngineInputError>>) -> (Vec<Payload>,Option<EngineInputError>) {
    let mut all:Vec<Result<Payload, EngineInputError>> = s.collect().await;
    let reason = match all.last() {
        None => None,
        Some(Err(e)) => all.pop().unwrap().err(),
        Some(Ok(..)) => None
    };
    return (
        all.into_iter().filter_map(|r| r.ok()).collect(),
        reason
    )
}




