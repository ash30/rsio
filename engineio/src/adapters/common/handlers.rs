use futures_util::Stream;
use crate::{EngineCloseReason, MessageData};
use super::emitter::AsyncEmitter;

pub trait NewConnectionService {
    fn new_connection<S:Stream<Item=Result<MessageData,EngineCloseReason>> + 'static>(&self, stream:S, emit:AsyncEmitter);
}
