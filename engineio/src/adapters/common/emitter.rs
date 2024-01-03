use tokio::sync::mpsc::Sender;
use crate::engine::{Payload };

pub enum EmitterError {
    Failed
}

pub struct Emitter {
    pub tx: Sender<Payload>,
}   

impl Emitter {
    pub async fn send(&self, p:Payload) -> Result<(),EmitterError> {
        self.tx.send(p).await.map_err(|_e| EmitterError::Failed)
    }
}

