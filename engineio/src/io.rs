use futures_util::{Stream, pin_mut};
use std::fmt;

use crate::engine::*;

// ==============================

use tokio_stream::StreamExt;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::Receiver;
use tokio_stream::wrappers::ReceiverStream;

pub fn create_poll_session<S1,S2,T>(client_events:S1, server_events:S2 ) -> (Sid, impl Stream<Item = Payload>,  Receiver<Payload>)
where S1: Stream<Item = T> + Send + 'static,
      S2: Stream<Item = T> + Send + 'static,
      T: TransportEvent + Send,
{
    let (client_forward_tx, client_forward_rx) = tokio::sync::mpsc::channel(10);
    let (server_forward_tx, server_forward_rx) = tokio::sync::mpsc::channel(10);
    let mut engine = Engine::new();
    let sid = engine.session;

    tokio::spawn(async move {
        let events = client_events.
            map(|v| Participant::Client(v))
            .merge(
                server_events.map(|v| Participant::Server(v))
            );

        pin_mut!(events);

        while let Some(v) = events.next().await {
            // for each received transport event, pass it up to consumer 
            engine.consume_transport_event(v);
            let res:std::result::Result<(), SendError<Payload>> = loop {
                let err = match engine.poll_output() {
                    None => break Ok(()),
                    Some(Participant::Client(p)) => client_forward_tx.send(p).await,
                    Some(Participant::Server(p)) => server_forward_tx.send(p).await
                };
                if let Err(..) = err { break err } 
            };
            if let Err(..) = res { break } 
        }
    });

    (sid, ReceiverStream::new(client_forward_rx), server_forward_rx)
}   
