use std::collections::VecDeque;

use crate::{Sid, EngineOutput, EngineState};

pub (crate) struct EngineIOClient {
    pub session: Sid,
    output: VecDeque<EngineOutput>,
    state: EngineState
}



