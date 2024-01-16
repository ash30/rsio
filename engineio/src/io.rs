use std::time::Duration;
use std::time::Instant;
use std::collections::HashMap;
use futures_util::Stream;
use futures_util::StreamExt;

use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use crate::EngineInput;
use crate::EngineOutput;
use crate::EngineError;
use crate::TransportConfig;
use crate::engine;
use crate::engine::{Sid, Payload, Engine, Participant} ;

type ForwardingChannel<IN,OUT> = (Sender<IN>, Receiver<OUT>);


// INTERFACE: 
// NEW SESSION ( SID ) 
//
// CLEITN NEED x1 TX, leave RX 
// SERVEFR SEND x1 
//
// NEW SESION ( TX) => GIVE RX TO STREAM in HANDLER 
//
// POLL( RX) 
type AsyncIOSender = Sender<Result<Payload,EngineError>>;

pub fn async_session_io_create() -> AsyncSessionIOHandle {
    let (client_sent_tx, mut client_sent_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncIOSender>)>(1024);
    let (server_sent_tx, mut server_sent_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncIOSender>)>(1024);
    let (time_tx, mut time_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncIOSender>)>(1024);

    let mut engines: HashMap<Sid,Engine> = HashMap::new();
    let mut server_recv: HashMap<Sid, AsyncIOSender> = HashMap::new();
    let mut client_recv: HashMap<Sid, AsyncIOSender> = HashMap::new();

    dbg!();
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
                    Ok(engines.get_mut(&sid).unwrap())
                },

                (EngineInput::New(..), Some(..)) => Err(EngineError::UnknownSession),

                (EngineInput::Listen(participant), Some(engine)) => {
                    match participant {
                        Participant::Client => client_recv.insert(sid, output.clone().unwrap()),
                        Participant::Server => server_recv.insert(sid, output.clone().unwrap()),
                    };
                    Ok(engine)
                }

                (_, None) => Err(EngineError::UnknownSession),

                (_, Some(engine)) => Ok(engine)
            };

            dbg!(sid);
            dbg!("{}",engine.is_ok());

            match engine {
                Ok(engine) => {
                    engine.consume(input, Instant::now());
                    let wait = loop {
                        let out = engine.poll_output();
                        dbg!(&out);
                        match out {
                            Some(EngineOutput::Pending(duration,id)) => break Some((duration,id)),
                            Some(EngineOutput::Data(Participant::Client,p)) => { 
                                let t = vec![output.as_ref(), server_recv.get(&sid)];
                                for tx in t.into_iter().filter_map(|a| a) {
                                    tx.send(Ok(p.clone())).await;
                                }
                                dbg!();
                            },
                            Some(EngineOutput::Data(Participant::Server,p)) => {
                                // TODO: UNWRAP 
                                let t = vec![output.as_ref(), client_recv.get(&sid)];
                                for tx in t.into_iter().filter_map(|a| a) {
                                    tx.send(Ok(p.clone())).await;
                                }
                                dbg!();
                            },
                            Some(EngineOutput::Closed(..)) => {
                                // .. what todo
                                if let Some(output) = &output {
                                    output.send(Err(EngineError::Generic)).await;
                                }
                            },
                            None => {
                                //... what todo...
                                break None
                            }
                        }
                    };
                    dbg!();
                    // Schedule next tick
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
                },
                Err(e) => {
                    dbg!(e);
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




