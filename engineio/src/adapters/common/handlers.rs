use futures_util::Stream;
use crate::Payload;
use super::emitter::Emitter;

pub trait NewConnectionService {
    fn new_connection<S:Stream<Item=Payload> + 'static>(&self, stream:S, emit:Emitter);
}
