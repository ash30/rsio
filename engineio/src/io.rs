use futures_util::Stream;
use crate::engine::*;

pub struct AsyncEngineInner<T:Transport, S:Stream>  {
    rx:S,
    engine: Engine<T>
}

impl <T:Transport, S:Stream> AsyncEngineInner<T,S> { 
    pub fn new(engine:Engine<T>,stream:S) -> Self {
        return Self { rx: stream, engine }
    }
}


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

