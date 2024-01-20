use std::time::Instant;
use std::collections::HashMap;
use futures_util::Stream;

use futures_util::StreamExt;
use tokio::sync::mpsc::Sender;
use crate::EngineInputError;
use crate::EngineInput;
use crate::EngineOutput;
use crate::engine::{Sid, Payload, Engine, Participant} ;

type AsyncIOSender = Sender<Result<Payload,EngineInputError>>;

type AsyncIOInput = tokio::sync::oneshot::Sender<Result<Option<Sender<Payload>>,EngineInputError>>;

pub fn async_session_io_create() -> AsyncSessionIOHandle {
    let (client_sent_tx, mut client_sent_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncIOInput>)>(1024);
    let (server_sent_tx, mut server_sent_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncIOInput>)>(1024);
    let (time_tx, mut time_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput, Option<AsyncIOInput>)>(1024);

    let mut engines: HashMap<Sid,Engine> = HashMap::new();
    let mut server_recv: HashMap<Sid, AsyncIOInput> = HashMap::new();
    let mut client_recv: HashMap<Sid, AsyncIOInput> = HashMap::new();

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

            let output = engine.ok_or(EngineInputError::AlreadyClosed)
                .and_then(|e| e.consume(input, Instant::now()).and(Ok(e)))
                .and_then(|e| { let mut b = vec![]; while let Some(p) = e.poll_output() { b.push(p); }; Ok(b) });

            match output {
                Err(e) => {
                    if let Some(t) = tx {
                        t.send(Err(e));
                    }
                },
                Ok(output) => {
                    for o in output {
                        match o {
                            EngineOutput::Data(participant, data) => {
                                let sender = if let Participant::Client = participant {server_recv.get(&sid)} else { client_recv.get(&sid) };
                                if let Some(sender) = sender {
                                    sender.send(Ok(data));
                                };
                                None
                            },
                            EngineOutput::SetIO(participant, close ) => {
                                let map = if let Participant::Server = participant { server_recv } else { client_recv };
                                let (tx,rx) = tokio::sync::mpsc::channel::<Payload>(10);
                                map.insert(sid, tx);
                                Some(rx)
                            },
                            EngineOutput::Tick { length } => {
                                let tx = time_tx.clone();
                                tokio::spawn(async move { 
                                    tokio::time::sleep(length).await; 
                                    tx.send((sid, EngineInput::Tock, None)).await
                                });
                                None
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
    client_send_tx: Sender<(Sid, EngineInput, Option<AsyncIOInput>)>,
    server_send_tx: Sender<(Sid, EngineInput, Option<AsyncIOInput>)>,
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

pub async fn session_stream(s: impl Stream<Item=Result<Payload,EngineInputError>> ) -> Result<impl Stream<Item=Payload>,EngineInputError> { 
    let mut stream = Box::pin(s);
    match stream.next().await {
        None => Err(EngineInputError::AlreadyClosed),
        Some(Err(e)) => Err(e.clone()),
        Some(Ok(p)) => Ok(stream.filter_map(|a| async move { a.ok() }))
    }
}




