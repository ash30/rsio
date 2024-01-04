use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use crate::engine::{Sid, Payload, Engine, Participant} ;
use crate::proto::TransportError;

type ForwardingChannel<T,E> = (Sender<Result<T,E>>, Receiver<T> );
pub struct SessionIO<T,E>(pub ForwardingChannel<T,E>,pub ForwardingChannel<T,E>);

pub fn create_session() -> (Sid,SessionIO<Payload,TransportError>) {

    let mut engine = Engine::new();
    let sid = engine.session;

    let (client_sent_tx, mut client_sent_rx) = tokio::sync::mpsc::channel::<Result<Payload,TransportError>>(10);
    let (server_sent_tx, mut server_sent_rx) = tokio::sync::mpsc::channel::<Result<Payload,TransportError>>(10);

    let (client_forward_tx, client_forward_rx) = tokio::sync::mpsc::channel(10);
    let (server_forward_tx, server_forward_rx) = tokio::sync::mpsc::channel(10);

    tokio::spawn(async move {
        loop {
            let input = tokio::select! {
                v1 = client_sent_rx.recv() => v1.and_then(|p| Some(Participant::Client(p))),
                v2 = server_sent_rx.recv() => v2.and_then(|p| Some(Participant::Server(p)))
            };

            if let Some(res) = input {
                engine.consume_transport_event(res);
                let err = match engine.poll_output() {
                    None => Ok(()),
                    Some(Participant::Client(p)) => client_forward_tx.send(p).await,
                    Some(Participant::Server(p)) => server_forward_tx.send(p).await
                };

                if let Err(_e) = err { break } 
            }
            else {
                break // one of the input channels has closed 
            }
        };
        // CLOSE INPUTS and outputs
        client_sent_rx.close();
        server_sent_rx.close();
        drop(server_forward_tx);
        drop(client_forward_tx);
    });

    let io = SessionIO(
        (client_sent_tx, client_forward_rx),
        (server_sent_tx, server_forward_rx),
    );

    return (sid,io);
}


