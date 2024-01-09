use std::sync::mpsc::TryRecvError;

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
            match session_rx.recv().await {
                Err(..) => break,
                Ok(Payload::Noop) => break,
                Ok(p) => {body.push(p);}
            };
        }
        return Ok(body);
    }

    pub async fn post(&self, sid: Option<uuid::Uuid>, body: Vec<u8>) -> Result<(),EngineError> {
        let sid = sid.ok_or(EngineError::UnknownSession)?;
        let tx = self.writers.get(&sid).ok_or(EngineError::UnknownSession)?;


        let res = tx.send_timeout(EngineInput::Data(Participant::Client,Payload::Message(body)), Duration::from_millis(1000) ).await;

        return res.map_err(|e| match e {
            mpsc::error::SendTimeoutError::Closed(..) => EngineError::SessionAlreadyClosed,
            mpsc::error::SendTimeoutError::Timeout(..) => EngineError::SessionUnresponsive
        })
    }
}


