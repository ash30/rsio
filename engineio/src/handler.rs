use crate::proto::MessageData;
use crate::engine::EngineCloseReason;

pub type ConnectionMessage = Result<MessageData,EngineCloseReason>;

