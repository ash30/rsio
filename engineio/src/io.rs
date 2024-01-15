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


pub fn async_session_io_create() -> AsyncSessionIOHandle {
    let (client_sent_tx, mut client_sent_rx) = tokio::sync::mpsc::channel::<(Sid,EngineInput, Option<Sender<Payload>>)>(10);
    let (server_sent_tx, mut server_sent_rx) = tokio::sync::mpsc::channel::<(Sid,EngineInput, Option<Sender<Payload>>)>(10);
    let (time_tx, mut time_rx) = tokio::sync::mpsc::channel::<(Sid,EngineInput, Option<Sender<Payload>>)>(10);

    let mut engines: HashMap<Sid,Engine> = HashMap::new();
    let mut server_recv: HashMap<Sid,Sender<Payload>> = HashMap::new();
    let mut client_recv: HashMap<Sid,Sender<Payload>> = HashMap::new();

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
                            EngineOutput::Pending(duration,id) => break Some((duration,id)),
                            EngineOutput::Data(Participant::Client,p) => { 
                                let t = vec![output.as_ref(), server_recv.get(&sid)];
                                for tx in t.into_iter().filter_map(|a| a) {
                                    tx.send(p.clone()).await;
                                }
                                dbg!();
                            },
                            EngineOutput::Data(Participant::Server,p) => {
                                // TODO: UNWRAP 
                                let t = vec![output.as_ref(), client_recv.get(&sid)];
                                for tx in t.into_iter().filter_map(|a| a) {
                                    tx.send(p.clone()).await;
                                }
                                dbg!();
                            },
                            EngineOutput::Closed(..) => break None
                        }
                    };
                    dbg!();
                    if let Some((wait,id)) = wait {
                        let tx = time_tx.clone();
                        tokio::spawn( async move {
                            tokio::time::sleep(wait).await; 
                            if let Some(polling_id) = id {
                                let _ = tx.send((sid, EngineInput::Poll(polling_id),output)).await;
                            }
                            else {
                                let _ = tx.send((sid,EngineInput::NOP, output)).await;
                            }
                        });
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
    client_send_tx: Sender<(Sid, EngineInput, Option<Sender<Payload>>)>,
    server_send_tx: Sender<(Sid, EngineInput, Option<Sender<Payload>>)>,
}

impl AsyncSessionIOHandle {

    pub async fn input(&self, id:Sid, input:EngineInput) -> impl Stream<Item=Payload> {
        let (tx,rx) = tokio::sync::mpsc::channel::<Payload>(10);
        let _ = self.client_send_tx.send((id,input, Some(tx))).await;
        return tokio_stream::wrappers::ReceiverStream::new(rx);
    }

    pub async fn new(&self, id:Sid, config:TransportConfig) -> impl Stream<Item=Payload> {
        dbg!();
        let (tx,rx) = tokio::sync::mpsc::channel::<Payload>(10);
        //let _ = self.client_send_tx.send((id, EngineInput::New(Some(config)), Some(tx))).await;
        return tokio_stream::wrappers::ReceiverStream::new(rx);
    }

    pub async fn listen(&self, id:Sid, filter:Participant) -> impl Stream<Item=Payload> {
        dbg!();
        let (tx,rx) = tokio::sync::mpsc::channel::<Payload>(10);
        let _ = self.client_send_tx.send((id, EngineInput::Listen(filter), Some(tx))).await;
        return tokio_stream::wrappers::ReceiverStream::new(rx);
    }

    pub async fn client_send(&self, id:Sid, payload:Payload) {
        self.client_send_tx.send((id, EngineInput::Data(Participant::Client, payload), None)).await;
    }

    pub async fn server_send(&self, id:Sid, payload:Payload) {
        self.server_send_tx.send((id, EngineInput::Data(Participant::Server, payload), None)).await;
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
       self.handle.server_send(self.sid, payload).await;
   }
}




