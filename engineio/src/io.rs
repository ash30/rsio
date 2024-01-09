use std::time::Instant;

use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc::Sender;
use crate::EngineInput;
use crate::EngineOutput;
use crate::engine::{Sid, Payload, Engine, Participant} ;

type ForwardingChannel<IN,OUT> = (Sender<IN>, Receiver<OUT>);

pub fn create_session_async() -> (Sid, (ForwardingChannel<EngineInput, Payload>, ForwardingChannel<EngineInput, Payload>)) {

    let mut engine = Engine::new();
    let sid = engine.session;

    let (client_sent_tx, mut client_sent_rx) = tokio::sync::mpsc::channel::<EngineInput>(10);
    let (server_sent_tx, mut server_sent_rx) = tokio::sync::mpsc::channel::<EngineInput>(10);

    let (client_forward_tx, client_forward_rx) = tokio::sync::broadcast::channel::<Payload>(10);
    let (server_forward_tx, server_forward_rx) = tokio::sync::broadcast::channel::<Payload>(10);

    tokio::spawn(async move {
        loop {
            let wait = loop {
                match engine.poll_output() {
                    EngineOutput::Pending(duration) => break Some(duration),
                    EngineOutput::Data(Participant::Client,p) => { client_forward_tx.send(p); },
                    EngineOutput::Data(Participant::Server,p) => { server_forward_tx.send(p); },
                    EngineOutput::Closed(..) => break None
                }
            };
            if let Some(time) = wait {
                let input = tokio::select! {
                    v1 = client_sent_rx.recv() => v1.unwrap_or(EngineInput::Close(Participant::Client)),
                    // TODO: SHOULD WE ALLOW THEM TO DROP THE EMITTER?
                    v2 = server_sent_rx.recv() => v2.unwrap_or(EngineInput::Close(Participant::Server)),
                    timeout = tokio::time::sleep(time) => EngineInput::NOP,
                };
                engine.consume(input, Instant::now());
            }
            else { 
                break 
            }
        }

        // CLOSE INPUTS and outputs
        client_sent_rx.close();
        server_sent_rx.close();
        drop(server_forward_tx);
        drop(client_forward_tx);
    });

    return (sid, ((client_sent_tx, client_forward_rx), (server_sent_tx, server_forward_rx)));
}


