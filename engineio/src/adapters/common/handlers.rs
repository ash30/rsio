use futures_util::Stream;
use crate::{Payload, EngineInputError};
use super::emitter::Emitter;

pub trait NewConnectionService {
    fn new_connection<S:Stream<Item=Result<Payload,EngineInputError>>+ 'static>(&self, stream:S, emit:Emitter);
}
