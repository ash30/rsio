use futures_util::Stream;
use tokio::sync::broadcast::error::RecvError;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use crate::Payload;
use super::emitter::Emitter;

pub trait NewConnectionService {
    fn new_connection<S:Stream<Item=Payload>+ 'static>(&self, stream:S, emit:Emitter);
}
