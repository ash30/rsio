use futures_util::Stream;
use super::emitter::Emitter;

pub trait NewConnectionService {
    fn new_connection<S:Stream<Item=Vec<u8>> + 'static>(&self, stream:S, emit:Emitter);
}
