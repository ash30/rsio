use futures_util::Stream;
use crate::{Payload, EngineInputError, EngineCloseReason};
use super::emitter::Emitter;

pub trait NewConnectionService {
    fn new_connection<S:Stream<Item=Result<Vec<u8>,EngineCloseReason>> + 'static>(&self, stream:S, emit:Emitter);
}
