
use dashmap::DashMap;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio::time::Duration;

use futures_util::Stream;
use std::fmt;
use crate::*;

pub trait NewConnectionService {
    fn new_connection<S:Stream<Item=Vec<u8>> + 'static, F:Fn(Vec<u8>)>(&self, stream:S, emit:F);
}

// ================

#[derive(Debug)]
pub enum SessionError {
    UnknownSession,
    SessionClosed,
    MultipleInflightPollRequest,
    SessionUnresponsive,
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self)
    }
}

pub type Result<T> = std::result::Result<T,SessionError>;

// ================

pub struct LongPollRouter {
    pub readers:DashMap<Sid, Mutex<mpsc::Receiver<Payload>>>,
    pub writers:DashMap<Sid, mpsc::Sender<LongPollEvent>>
}

impl LongPollRouter {
    pub fn new() -> Self {
        return Self { 
            readers: DashMap::new(),
            writers: DashMap::new(),
        }
    }

    pub async fn poll_session(&self, sid:Option<uuid::Uuid>) -> Result<proto::Payload> {
        let sid = sid.ok_or(SessionError::UnknownSession)?;
        let session = self.readers.get(&sid).ok_or(SessionError::UnknownSession)?;

        // We have to assign guard here OTHERWISE, as a temp, it will be released AFTER session
        // which it relies on. By binding it, we force its scope to be non temp
        //
        // https://stackoverflow.com/questions/53586321/why-do-i-get-does-not-live-long-enough-in-a-return-value
        //
        // https://stackoverflow.com/questions/65972165/why-is-the-temporary-is-part-of-an-expression-at-the-end-of-a-block-an-error
        //
        let x = if let Ok(mut rx) = session.try_lock() {
            let events = rx.recv().await.ok_or(SessionError::SessionClosed)?;
            return Ok(events)
        }
        else {
            Err(SessionError::MultipleInflightPollRequest)
        }; x
    }

    pub async fn post_session(&self, sid: Option<uuid::Uuid>, body: Vec<u8>) -> Result<()> {

        let sid = sid.ok_or(SessionError::UnknownSession)?;
        let session = self.writers.get(&sid).ok_or(SessionError::UnknownSession)?;

        let res = session.send_timeout(LongPollEvent::POST(body), Duration::from_millis(1000) ).await;
        return res.map_err(|e| match e {
            mpsc::error::SendTimeoutError::Closed(..) => SessionError::SessionClosed,
            mpsc::error::SendTimeoutError::Timeout(..) => SessionError::SessionUnresponsive
        })
    }
}


