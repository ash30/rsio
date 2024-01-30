use crate::{MessageData, EngineCloseReason};

pub type ConnectionMessage = Result<MessageData,EngineCloseReason>;

