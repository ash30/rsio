use crate::engine::{Participant, EngineInput};
use crate::proto::{Sid, Payload, MessageData};
use crate::io::AsyncIOHandle;

pub struct AsyncSessionIOSender {
    sid:Sid,
    handle: AsyncIOHandle
}

impl AsyncSessionIOSender {
    pub fn new(sid:Sid, handle:AsyncIOHandle) -> Self {
        Self {
            sid, handle
        }
    }

   pub async fn send(&self, data:MessageData) {
       // TODO: Listen for SEND errors ?
       self.handle.input(
           self.sid, 
           EngineInput::Data(Participant::Server, Ok(Payload::Message(data))),
        ).await;
   }

}

pub type AsyncEmitter = AsyncSessionIOSender;
