use std::sync::mpsc::TryRecvError;

use actix_web::body;
use dashmap::DashMap;
use tokio::sync::{mpsc, broadcast};
use tokio::time::Duration;

use crate::{EngineInput, Participant};
use crate::engine::{Sid, Payload};
use crate::proto::EngineError;

pub struct LongPollRouter {
    pub readers:DashMap<Sid, broadcast::Receiver<Payload>>,
    pub writers:DashMap<Sid, mpsc::Sender<EngineInput>>
}

use tokio::time::timeout;


impl LongPollRouter {
    pub fn new() -> Self {
        return Self { 
            readers: DashMap::new(),
            writers: DashMap::new(),
        }
    }

    pub async fn poll(&self, sid:Option<uuid::Uuid>) -> Result<Vec<Payload>,EngineError> {
        let sid = sid.ok_or(EngineError::UnknownSession)?;
        let mut session_rx = self.readers.get_mut(&sid).ok_or(EngineError::UnknownSession)?;
        let session_tx = self.writers.get(&sid).ok_or(EngineError::UnknownSession)?;
        let res = session_tx.send(EngineInput::Poll).await;
        let mut body = vec![];
        loop {
            println!("foo");
            match session_rx.recv().await {
                Err(..) => break,
                Ok(Payload::Noop) => break,
                Ok(p) => {body.push(p);}
            };
        }
            println!("foo2");
        return Ok(body);
    }

    pub async fn post(&self, sid: Option<uuid::Uuid>, body: Vec<u8>) -> Result<(),EngineError> {
        let sid = sid.ok_or(EngineError::UnknownSession)?;
        let tx = self.writers.get(&sid).ok_or(EngineError::UnknownSession)?;
        
        // parse messages
        let mut buf = vec![];
        let mut start = 0;
        let mut iter = body.iter().enumerate();
        while let Some(d) = iter.next() {
            let (n,data) = d;
            if *data == b"\x1e"[0] {
                buf.push(&body[start..n]);
                start = n+1;
                println!("test1b");
            }
        }
         buf.push(&body[start..body.len()]);

        for msg in buf.iter().filter(|v| v[0] == b"4"[0] ) {
            // TODO:ollect errors...
            let res = tx.send_timeout(EngineInput::Data(Participant::Client,Payload::Message((**msg)[1..].to_vec())), Duration::from_millis(1000) ).await;
        }
        Ok(())
    }
}


