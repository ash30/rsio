use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use crate::EngineInput;
use crate::EngineOutput;
use crate::engine::{Sid, Payload, Engine, Participant} ;

type ForwardingChannel<IN,OUT> = (Sender<IN>, Receiver<OUT>);
pub struct AsyncSession(pub ForwardingChannel<EngineInput, Payload>,pub ForwardingChannel< EngineInput, Payload>);


pub fn create_session() -> (Sid, AsyncSession) {

    let mut engine = Engine::new();
    let sid = engine.session;

    let (client_sent_tx, mut client_sent_rx) = tokio::sync::mpsc::channel::<EngineInput>(10);
    let (server_sent_tx, mut server_sent_rx) = tokio::sync::mpsc::channel::<EngineInput>(10);

    let (client_forward_tx, client_forward_rx) = tokio::sync::mpsc::channel::<Payload>(10);
    let (server_forward_tx, server_forward_rx) = tokio::sync::mpsc::channel::<Payload>(10);

    tokio::spawn(async move {
        loop {
            let input = tokio::select! {
                v1 = client_sent_rx.recv() => v1,
                v2 = server_sent_rx.recv() => v2
            };
            if let Some(res) = input {
                engine.consume(res);
                match engine.poll_output() {
                    None => Ok(()),
                    Some(EngineOutput::Pending) => Ok(()),
                    Some(EngineOutput::Data(Participant::Client, p )) => client_forward_tx.send(p).await,
                    Some(EngineOutput::Data(Participant::Server, p )) => server_forward_tx.send(p).await,
                    Some(EngineOutput::Closed(..)) => break 
                };
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

    let io = AsyncSession(
        (client_sent_tx, client_forward_rx),
        (server_sent_tx, server_forward_rx),
    );

    return (sid,io);
}


