use futures_util::Stream;
use tokio_stream::StreamExt;
use crate::{EngineCloseReason, MessageData, Payload};
use super::emitter::AsyncEmitter;

pub trait NewConnectionService {
    fn new_connection<S:Stream<Item=Result<MessageData,EngineCloseReason>> + 'static>(&self, stream:S, emit:AsyncEmitter);
}

pub(crate) fn create_connection_stream(s:impl Stream<Item=Payload>) -> impl Stream<Item=Result<MessageData,EngineCloseReason>> {
    s.filter_map(|p| match p { 
        Payload::Message(d) => Some(Ok(d)),
        Payload::Close(r) => Some(Err(r)),
        _ => None,
    })
}
