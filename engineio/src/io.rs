use std::fmt::Debug;
use std::time::Duration;
use std::time::Instant;
use futures_util::TryFutureExt;
use tokio::select;
use tokio::sync::mpsc::Receiver;
use crate::Engine;
use crate::EngineError;
use crate::EngineStateEntity;
use crate::TransportConfig;
use crate::client::EngineIOClient;
use crate::engine::{Sid, Payload, EngineInput, EngineIOClientCtrls, EngineIOServerCtrls, IO} ;
use crate::server::EngineIOServer;

pub enum IOError<T> {
    InternalError,
    SendError(T),
    TransportError(T)
}

pub enum IOCloseReason {}

use tokio::sync::oneshot;
use tokio::sync::mpsc;

pub(crate) type AsyncDownStreamIOPair = (
    mpsc::Sender<(Payload, oneshot::Sender<Result<(),IOError<Payload>>>)>,
    mpsc::Receiver<(Result<Payload,IOCloseReason>, oneshot::Sender<Result<(),EngineError>>)>
    );

pub(crate) type AsyncDownStreamIOPairOther = (
    mpsc::Receiver<(Payload, oneshot::Sender<Result<(),IOError<Payload>>>)>,
    mpsc::Sender<(Result<Payload,IOCloseReason>, oneshot::Sender<Result<(),EngineError>>)>
    );

type AsyncUpStreamIOPair = (
    mpsc::Sender<Result<Payload,IOCloseReason>>, // change to engine close reason
    mpsc::Receiver<(Payload, oneshot::Sender<Result<(),EngineError>>)>
    );

type AsyncUpStreamIOPairOTHER = (
    mpsc::Receiver<Result<Payload,IOCloseReason>>, // change to engine close reason
    mpsc::Sender<(Payload, oneshot::Sender<Result<(),EngineError>>)>
    );

pub trait Transport { 
    fn connect(&self) -> AsyncDownStreamIOPair;
}

pub fn setup_client_io(t:impl Transport) -> AsyncUpStreamIOPairOTHER {
    let (up_send_tx, up_send_rx) = tokio::sync::mpsc::channel(1);
    let (up_recv_tx, up_recv_rx) = tokio::sync::mpsc::channel(1);

    // TODO: We need to return used engine so people can drain?
    let task = setup_io(t.connect(), (up_send_tx, up_recv_rx));
    return (up_recv_tx, up_send_rx);
}

async fn setup_io(
    down_stream:AsyncDownStreamIOPair,
    up_stream:AsyncUpStreamIOPair
) -> Engine {
    let (down_send, down_recv) = down_stream;
    let (up_send, up_recv) = up_stream;

    let mut engine = Engine::new(EngineIOClient::new(tokio::time::Instant::now()));
    let config = TransportConfig::default();

    let a = tokio::spawn(async move { 
        loop {
            let config = TransportConfig::default();
            let now = tokio::time::Instant::now();        

            let next = loop {
                match engine.poll(now, &config) {
                    Some(IO::Wait(d)) => {
                        break Ok(Some(d))
                    }
                    Some(IO::Send(p)) => {
                        let (res_tx, res_rx) = tokio::sync::oneshot::channel::<Option<(IOError, Payload)>>();
                        if let Err(e) = down_send.send((res_tx,Payload::Ping)).await {
                            break Err(IOError::SendError(e.0))
                        }
                        match res_rx.await {
                            Err(e) => break Err(IOError::InternalError),
                            Ok(Err(e)) => break Err(e),
                            Ok(_) => continue 
                        }
                    },
                    Some(IO::Recv(p)) => {
                        // We don't mind if upstream has dropped
                        let _ = up_send.send(Ok(p)).await;
                    }
                    Some(_) => {
                        // we ingore others for now ...
                    }
                    None => {
                        break(Ok(None))
                    }
                }
            };

            let next_deadline = match next {
                Ok(None) => break, // engine closed 
                Ok(Some(d)) => Some(d),
                Err(e) => {
                    // TODO: Send errors back into the engine....
                    // HOW do we input error? CTRLs 
                    engine.input(e, now, &config);
                }
            };
            select! {
                _ = tokio::time::sleep_until(next_deadline.unwrap_or(tokio::time::Instant::now())) => {},
                up = up_recv.recv() => {
                    // TODO: dropping channel cleanup 
                    // We report invalid input to local sender
                    let Some((p,res_tx)) = up else { 
                        // IF upstream closes... what do do ?
                        break
                    };
                    res_tx.send(engine.input(p,now,&config)).await;
                },
                down = down_recv.recv() => {
                    // TODO: dropping channel cleanup 
                    // We report invalid input to remote sender
                    let Some((p,res_tx)) = down else { 
                        // when down stream closes, we can break right away
                        // and let owner drain ...
                        break 
                    };
                    res_tx.send(engine.input(p,now,&config)).await;
                    // Data - command to show stuff 
                    // POLL -> command from other side...
                    // New -> command from other side as well
                    // Close -> THIS
                    // data stream has close in it, close at this level is for server,client??;
                }
            };
        };
        engine
    });

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





