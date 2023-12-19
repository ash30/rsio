use futures_util::Stream;
use std::fmt;

use crate::engine::*;


pub trait NewConnectionService {
    // TODO: WHY does it need to be static ??
    fn new_connection<S:Stream + 'static >(&self, stream:S);
}


pub struct AsyncEngineInner<T:Transport, S:Stream>  {
    rx:S,
    engine: Engine<T>
}

impl <T:Transport, S:Stream> AsyncEngineInner<T,S> { 
    pub fn new(engine:Engine<T>,stream:S) -> Self {
        return Self { rx: stream, engine }
    }
}


// WHERE Stream items can be converted into TRANSPORT events
impl <T:Transport, S:Stream<Item = U>, U:Into<T::Event>> Stream for AsyncEngineInner<T,S> 
where T: std::marker::Unpin,
      S: std::marker::Unpin,
      S::Item: Into<T::Event>
{
    type Item = Vec<u8>;    

    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let res = match this.engine.poll_output() {
            Output::Send(e) =>                                                    std::task::Poll::Pending, // SHOULD NOT HAPPEN ???
            Output::TransportChange(TransportState::Connected) =>       std::task::Poll::Pending,
            Output::TransportChange(TransportState::Disconnected) =>    std::task::Poll::Ready(None),
            Output::TransportChange(TransportState::Closed) =>          std::task::Poll::Ready(None),
            Output::Receive(Payload::Message(e)) =>                               std::task::Poll::Ready(Some(e)),
            Output::Receive(e) =>                                                 std::task::Poll::Pending,
            Output::Pending =>                                                    std::task::Poll::Pending
        };

        if let std::task::Poll::Pending = res { 
            if let std::task::Poll::Ready(Some(e)) = std::pin::Pin::new(&mut this.rx).poll_next(cx) {
            //if let std::task::Poll::Ready(Some(e)) = this.rx.poll_recv(cx) {
                this.engine.consume_transport_event(e.into());
                cx.waker().wake_by_ref();
            }
        }
        return res
    }
}

// ==============================

use dashmap::DashMap;
use tokio::sync::mpsc;
use tokio::time::Duration;


use crate::*;

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

pub async fn poll_session(
    sid:Option<uuid::Uuid>,
    sessions:&DashMap<Sid, tokio::sync::Mutex<mpsc::Receiver<Payload>>>) -> Result<proto::Payload>{
    
    let sid = sid.ok_or(SessionError::UnknownSession)?;
    let session = sessions.get_mut(&sid).ok_or(SessionError::UnknownSession)?;

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

pub async fn post_session(
    sid: Option<uuid::Uuid>,
    body: Vec<u8>,
    sessions:&DashMap<Sid, mpsc::Sender<LongPollEvent>>) -> Result<()> {

    let sid = sid.ok_or(SessionError::UnknownSession)?;
    let session = sessions.get(&sid).ok_or(SessionError::UnknownSession)?;

    let res = session.send_timeout(LongPollEvent::POST(body), Duration::from_millis(1000) ).await;
    // TODO: Look into Send Errors
    return res.map_err(|e| match e {
        mpsc::error::SendTimeoutError::Closed(..) => SessionError::SessionClosed,
        mpsc::error::SendTimeoutError::Timeout(..) => SessionError::SessionUnresponsive
    })
}


pub fn create_session() -> (Sid, AsyncEngineInner<LongPoll, impl Stream<Item = LongPollEvent>>, mpsc::Sender<LongPollEvent>) {
    let eng = Engine::new_longpoll();
    let sid = eng.session;
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let sio = AsyncEngineInner::new(eng, tokio_stream::wrappers::ReceiverStream::new(rx)); 
    return (sid, sio, tx)
}


