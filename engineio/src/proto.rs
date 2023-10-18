pub enum EngineKind {
    WS,
    LongPoll
}

pub enum TransportState { 
    Connected,
    Disconnected,
    Closed
}

pub enum Payload {
    Open,
    Close,
    Ping,
    Pong,
    Message(Vec<u8>),
    Upgrade,
    Noop
}

pub struct SessionConfig { 
    pub ping_interval: u32,
    pub ping_timeout: u32,
    pub max_payload: u32
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            ping_interval:25000,
            ping_timeout: 20000,
            max_payload: 1000000,
        }
    }
}
