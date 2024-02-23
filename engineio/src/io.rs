use std::fmt::Debug;
use std::time::Duration;
use std::time::Instant;
use std::collections::HashMap;
use futures_util::Stream;
use tokio_stream::StreamExt;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::Receiver;
use crate::Engine;
use crate::EngineError;
use crate::EngineCloseReason;
use crate::TransportKind;
use crate::EngineStateEntity;
use crate::MessageData;
use crate::TransportConfig;
use crate::TransportError;
use crate::client::EngineIOClient;
use crate::engine::{Sid, Payload, EngineInput, EngineIOClientCtrls, EngineIOServerCtrls, IO} ;
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


// THE AIM here is to wrap engine client inside of some IO Primitive
// I think TASK is still relevant - because we need to time things
// and move drive things forward.... and deliver callbacks based 
//
// Create Cliemt()
// client.connect().await -> Stream + Session ?
// 


async fn create_client_session(transport:impl AsyncTransportClient, kind:TransportKind)  -> Result<impl Stream<Item=Payload>,TransportError>{ // need to return session + stream 

    let (tx,rx) = tokio::sync::mpsc::channel(1024);
    let mut engine = Engine::new(EngineIOClient::new(Instant::now()));

    tokio::spawn(async move {
        let mut session = None;
        let mut stream = None;
        loop {
            let now = Instant::now();
            let config = TransportConfig::default();
            let wait = loop {
                match engine.poll(now, &config) {
                    Some(output) => match output {
                        IO::Connect(sid) => {
                            let (session, stream) = transport.connect(sid, kind).await;
                            session = session;
                            stream = stream;
                        },
                        IO::Close => { transport.close() }
                        IO::Send(p) => {
                            if let Some(s) = session {
                                s.send(p); //send to transport
                            }
                            else {
                                // HOW TO HNDLE ERROR???
                            }
                        },
                        IO::Recv(p) => { tx.send(p); }, // send to customer stream
                        IO::Wait(until) => break Some(until),
                   },
                   None => break None
               }
            };
            let Some(t) = wait else { break };
            let next = tokio::select! {
                _ = tokio::time::sleep_until(t.into()) => None,
                input = if let Some(s) = stream { s.next() } else { futures_util::future::pending() } => input 
            };
            if let Some(input) = next { engine.input(input, now, &config) }
        }
    });

    return Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
}

// WE WANT (SESSION + STREAM
// CONNECT() => Result<Session + STREAM >
//

pub(crate) struct AsyncSessionSENDER {}

pub (crate) trait AsyncTransportClient {

    async fn connect(&self, id:Option<Sid>, kind:TransportKind) -> Result<(AsyncSessionSENDER,impl Stream<Item = Payload>), TransportError> ;
    fn close(&mut self);
    
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


#[derive(Debug)]
pub enum Either<A,B> {
    A(A),
    B(B)
}


type EngineServerInput = Either<EngineInput<EngineIOServerCtrls>, EngineInput<EngineIOClientCtrls>>;

pub fn create_async_io2<F>(client:F, config:TransportConfig) -> AsyncSessionClientHandle 
where F:AsyncConnectionService + 'static + Send
{

    let (client_send_tx, mut client_send_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput<EngineIOClientCtrls>, AsyncInputSender)>(1024);
    let (server_send_tx, mut server_send_rx) = tokio::sync::mpsc::channel::<(Sid, EngineInput<EngineIOServerCtrls>, AsyncInputSender)>(1024);
    let mut workers: HashMap<Sid,Sender<(EngineServerInput, AsyncInputSender)>> = HashMap::new();
    
    tokio::spawn( async move {
        let config = config;
        loop {
            let (sid,input,res_tx) = tokio::select! {
                Some(server) = server_send_rx.recv() => (server.0, Either::A(server.1), server.2),
                Some(client) = client_send_rx.recv() => (client.0, Either::B(client.1), client.2),
            };

            let worker = match input {
                Either::B(EngineInput::Control(EngineIOClientCtrls::New(..))) => {
                    // Channel for IO_DISPATCHER to talk to worker
                    let (worker_recv_tx, worker_recv_rx) = tokio::sync::mpsc::channel::<(EngineServerInput,AsyncInputSender)>(32);
                    workers.insert(sid, worker_recv_tx.clone());
                    // Channel for END_CLIENT to recv events 
                    let (server_recv_tx, server_recv_rx) = tokio::sync::mpsc::channel::<Payload>(10);
                    client.new_connection(
                        create_connection_stream(tokio_stream::wrappers::ReceiverStream::new(server_recv_rx)),
                        AsyncSessionServerHandle{ sid:sid.clone(), input_tx:server_send_tx.clone()  }
                    );

                    tokio::spawn(create_worker(sid, worker_recv_rx, server_recv_tx.clone(), config.clone()));
                    Some(worker_recv_tx)
                },

                _ => {
                    workers.get(&sid).map(|t| t.to_owned())
                }
            };

            if let Some(w) = worker {
               if let Err(e) = w.send_timeout((input,res_tx), Duration::from_secs(1)).await {
                   //TODO: HOW do we time this out?
                    //res_tx.send(Result::Err(EngineError::Generic));
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

async fn create_worker(id:Sid, mut rx:tokio::sync::mpsc::Receiver<(EngineServerInput,AsyncInputSender)>, tx: tokio::sync::mpsc::Sender<Payload>, config:TransportConfig) {

    let now = Instant::now();
    let mut engine = Engine::new(EngineIOServer::new(id, now));
    let mut send_buffer = vec![];
    let mut send_tx = None;
    let mut next_tick = Instant::now() + Duration::from_secs(10);
    loop {
        let now = Instant::now();
        let (input,res_tx) = tokio::select! {
            input = rx.recv() => if let Some(i) = input { Some(i) } else { break },
            _  = tokio::time::sleep_until(next_tick.into()) => None
        }.unzip();

        let err = input.and_then(|input| {
            engine.input(input, now, &config).err()
        });
        
        let mut new_stream = None;
        let next_deadline = loop {
            match engine.poll(now, &config) {
                Some(output) => {
                    match output {
                        IO::Stream(true) => { 
                            let (tx,rx) = tokio::sync::mpsc::channel(32);
                            for p in send_buffer.drain(0..){
                                tx.send(p).await;
                            }
                            send_tx = Some(tx);
                            new_stream = Some(rx);

                        },
                        IO::Stream(false) => {send_tx = None;},
                        IO::Recv(m) => { tx.send(m).await;},
                        IO::Send(p) => { 
                            if let Some(sender) = &send_tx {
                                sender.send(p).await;
                            }
                            else {
                                send_buffer.push(p);
                            }   
                        },
                        IO::Wait(d) => { break Some(d) }
                    }
                },
                None => break None// finished! 
            }
        };
        // Send result back to input provider
        if let Some(t) = res_tx { 
            let res = match (err, new_stream) {
                (Some(e), _ ) => Err(e),
                (None, Some(rx)) => Ok(Some(rx)),
                (None, None) => Ok(None)
            };
            t.send(res);
        };

        if let Some(nd) = next_deadline  { next_tick = nd; }
        else { break } // finished
    }
}


pub fn create_async_client(service:impl AsyncConnectionService +'static + Send) {

}





