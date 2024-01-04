use tokio::sync::mpsc::Sender;
use crate::{engine::{Payload }, TransportError};

pub enum EmitterError {
    Failed
}

pub struct Emitter {
    pub tx: Sender<Result<Payload,TransportError>>,
}   

impl Emitter {
    pub async fn send(&self, p:Payload) -> Result<(),EmitterError> {
        self.tx.send(Ok(p)).await.map_err(|_e| EmitterError::Failed)
    }
}

