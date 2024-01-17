use std::time::Instant;
use std::collections::HashMap;
use futures_util::Stream;

use futures_util::StreamExt;
use tokio::sync::mpsc::Sender;
use crate::EngineCloseReason;
use crate::EngineInput;
use crate::EngineOutput;
use crate::EngineError;
use crate::engine::{Sid, Payload, Engine, Participant} ;

type AsyncIOSender = Sender<Result<Payload,EngineError>>;


pub fn async_session_io_create() -> AsyncSessionIOHandle {
    let (client_sent_tx, mut client_sent_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncIOSender>)>(1024);
    let (server_sent_tx, mut server_sent_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncIOSender>)>(1024);
    let (time_tx, mut time_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncIOSender>)>(1024);

    let mut engines: HashMap<Sid,Engine> = HashMap::new();
    let mut server_recv: HashMap<Sid, AsyncIOSender> = HashMap::new();
    let mut client_recv: HashMap<Sid, AsyncIOSender> = HashMap::new();

    tokio::spawn( async move {
        loop {
            dbg!();
            let (sid, input, output)  = tokio::select! {
                Some(v1) = client_sent_rx.recv() => v1,
                Some(v2) = server_sent_rx.recv() => v2,
                Some(v3) = time_rx.recv() => v3
            };

            dbg!(&input);

            let engine = match (&input, engines.get_mut(&sid)) {
                (EngineInput::New(..), None) => {
                    let e = Engine::new(sid);
                    engines.insert(sid,e);
                    engines.get_mut(&sid)
                },
                (_, engine) => engine
            };

            // Setup listeners 
            match (&input,&engine,&output) {
                (EngineInput::Listen(Participant::Server),Some(..), Some(sender)) => { server_recv.insert(sid, sender.clone());},
                (EngineInput::Listen(Participant::Client),Some(..), Some(sender)) => { client_recv.insert(sid, sender.clone());}
                _ => {}
            }

            let wait = match engine.and_then(|e| Some(e.consume(input, Instant::now()).and(Ok(e)))) {
                Some(Ok(engine)) => {
                    loop {
                        let out = engine.poll_output();
                        match out {
                            Some(EngineOutput::Data(participant, payload)) => {
                                let sender = if let Participant::Server = participant {server_recv.get(&sid)} else { client_recv.get(&sid) };
                                for tx in [output.as_ref(), sender].into_iter().filter_map(|a| a) {
                                    tx.send(Ok(payload.clone())).await;
                                }
                            },
                            Some(EngineOutput::Closed(reason)) => {
                                // The Engine should only dispatch x1 Close 
                                // We have to close all long lived listeners
                                let t = vec![output.as_ref(), client_recv.get(&sid), server_recv.get(&sid)];
                                for tx in t.into_iter().filter_map(|a| a) {
                                    tx.send(Ok(Payload::Close(None))).await;
                                }
                                match reason {
                                    EngineCloseReason::Timeout => {},
                                    EngineCloseReason::Command(p) => {},
                                    EngineCloseReason::Error(e) => {
                                        if let Some(output) = &output {
                                           output.send(Err(e)).await;
                                        }
                                    }
                                }
                            },
                            Some(EngineOutput::Pending(duration,id)) => break Some((duration,id)),
                            None => break None
                        }
                    }
                },
                // Engine doesn't exist OR is closed already
                _ => {
                    if let Some(output) = &output {
                        output.send(Err(EngineError::UnknownSession)).await;
                    };
                    None
                },
            };
            // Schedule Next Tick
            match wait {
                None => {},
                Some((duration, Some(poll))) => {
                    let tx = time_tx.clone();
                    tokio::spawn(async move { 
                        tokio::time::sleep(duration).await; 
                        tx.send((sid, EngineInput::Poll(poll), output)).await
                    }) ;
                },
                Some((duration, None)) => {
                    drop(output);
                    let tx = time_tx.clone();
                    tokio::spawn(async move { 
                        tokio::time::sleep(duration).await; 
                        tx.send((sid, EngineInput::NOP, None)).await
                    });
                }
            }
        }
    });

    return AsyncSessionIOHandle {
        client_send_tx:client_sent_tx,
        server_send_tx:server_sent_tx,
    }
}

#[derive(Debug, Clone)]
pub struct AsyncSessionIOHandle {
    client_send_tx: Sender<(Sid, EngineInput, Option<Sender<Result<Payload,EngineError>>>)>,
    server_send_tx: Sender<(Sid, EngineInput, Option<Sender<Result<Payload, EngineError>>>)>,
}

impl AsyncSessionIOHandle {

    pub async fn input(&self, id:Sid, input:EngineInput) -> impl Stream<Item=Result<Payload,EngineError>> {
        let (tx,rx) = tokio::sync::mpsc::channel::<Result<Payload,EngineError>>(10);
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
       self.handle.input(self.sid, EngineInput::Data(Participant::Server, payload)).await;
   }
}

pub async fn session_collect(s:impl Stream<Item=Result<Payload,EngineError>>) -> (Vec<Payload>,Option<EngineError>) {
    let mut all:Vec<Result<Payload,EngineError>> = s.collect().await;
    let reason = match all.last() {
        None => None,
        Some(Err(e)) => all.pop().unwrap().err(),
        Some(Ok(..)) => None
    };
    dbg!(&reason);
    dbg!(&all);
    return (
        all.into_iter().filter_map(|r| r.ok()).collect(),
        reason
    )
}




