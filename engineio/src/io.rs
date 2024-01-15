use std::time::Duration;
use std::time::Instant;
use std::collections::HashMap;
use futures_util::Stream;
use futures_util::StreamExt;

use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use crate::EngineInput;
use crate::EngineOutput;
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

    tokio::spawn( async move {
        loop {
            let (sid, input, output)  = tokio::select! {
                Some(v1) = client_sent_rx.recv() => v1,
                Some(v2) = server_sent_rx.recv() => v2,
                Some(v3) = time_rx.recv() => v3
            };

            match (&input, engines.get_mut(&sid)) {
                (EngineInput::New(..), None) => {
                    let mut e = Engine::new();
                    server_recv.insert(sid, output.unwrap());
                    e.consume(input, Instant::now());
                },
                (EngineInput::New(..), Some(..)) => {
                    
                },
                (_, None) => {}
                (_, Some(engine)) => {
                    engine.consume(input, Instant::now());

                    if let Some(output) = output {
                        client_recv.insert(sid,output);
                    }

                    let wait = loop {
                        let out = engine.poll_output();
                        match out {
                            EngineOutput::Pending(duration) => break Some(duration),
                            EngineOutput::Data(Participant::Client,p) => { 
                                // TODO: UNWRAP 
                                let tx = server_recv.get(&sid).unwrap();
                                tx.send(p);
                            },
                            EngineOutput::Data(Participant::Server,p) => {
                                // TODO: UNWRAP 
                                let tx = client_recv.get(&sid).unwrap();
                                tx.send(p);
                            },
                            EngineOutput::Closed(..) => break None
                        }
                    };
                    if let Some(wait) = wait {
                        let tx = time_tx.clone();
                        tokio::spawn( async move {
                            tokio::time::sleep(wait).await; 
                            tx.send((sid,EngineInput::NOP, None));
                        });
                    }
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
    client_send_tx: Sender<(Sid, EngineInput, Option<Sender<Payload>>)>,
    server_send_tx: Sender<(Sid, EngineInput, Option<Sender<Payload>>)>,
}

impl AsyncSessionIOHandle {
    pub async fn new(&self, id:Sid) -> impl Stream<Item=Payload> {
        let (tx,rx) = tokio::sync::mpsc::channel::<Payload>(10);
        let _ = self.client_send_tx.send((id, EngineInput::New(None), Some(tx)));
        return tokio_stream::wrappers::ReceiverStream::new(rx);
    }

    pub async fn client_poll(&self, id:Sid) -> impl Stream<Item=Payload> {
        let (tx,rx) = tokio::sync::mpsc::channel::<Payload>(10);
        let _ = self.client_send_tx.send((id, EngineInput::Poll, Some(tx)));
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




