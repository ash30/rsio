use dashmap::DashMap;
use tokio::sync::{mpsc, Mutex};
use tokio::time::Duration;

use crate::{EngineInput, Participant};
use crate::engine::{Sid, Payload};
use crate::proto::EngineError;

pub struct LongPollRouter {
    pub readers:DashMap<Sid, Mutex<mpsc::Receiver<Payload>>>,
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

    pub async fn poll(&self, sid:Option<uuid::Uuid>) -> Result<Payload,EngineError> {
        let sid = sid.ok_or(EngineError::UnknownSession)?;
        let session = self.readers.get(&sid).ok_or(EngineError::UnknownSession)?;

        // We have to assign guard here OTHERWISE, as a temp, it will be released AFTER session
        // which it relies on. By binding it, we force its scope to be non temp
        //
        // https://stackoverflow.com/questions/53586321/why-do-i-get-does-not-live-long-enough-in-a-return-value
        //
        // https://stackoverflow.com/questions/65972165/why-is-the-temporary-is-part-of-an-expression-at-the-end-of-a-block-an-error
        //
        let x = if let Ok(mut rx) = session.try_lock() {
            return match timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Some(events)) => Ok(events),
                Ok(None) => Err(EngineError::SessionAlreadyClosed),
                Err(..) => Ok(Payload::Message(vec![]))
            }
        }
        else {
            Err(EngineError::InvalidPollRequest)
        }; x
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


