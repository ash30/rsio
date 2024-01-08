use tokio::sync::mpsc::Sender;
use crate::{engine::{Payload }, EngineInput};

pub enum EmitterError {
    Failed
}

pub struct Emitter {
    pub tx: Sender<EngineInput>,
}   

impl Emitter {
    pub async fn send(&self, p:Payload) -> Result<(),EmitterError> {
        self.tx.send(EngineInput::Data(crate::Participant::Server,p)).await.map_err(|_e| EmitterError::Failed)
    }
}

