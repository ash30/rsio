use std::{collections::{HashMap, VecDeque}};


pub enum Error { 
    UnknownInput,
    InvalidStateTransition,
    UnsupportedProtocolVersion
}



pub enum EngineTransport {
    None,
    Polling,
    Socket
}

enum EngineState { 
    New,
    Upgrading,
    Connected(EngineTransport),
}

pub enum ClientEvent {
    Connect { eio:u8, transport:EngineTransport },
    Upgrade
}

pub enum ServerEvent {
    NewConnection { 
        sid:String, upgrades:[EngineTransport;1], pingInterval:u32, pingTimeout:u32, maxPayload:u32 
    } 
}

pub enum EngineOutput {
    Wait, // nothing in buffer, wait ?
    Event(ServerEvent),
}

// ==========================

pub struct EngineConfig {
    pingInterval: u32,
    pingTimeout: u32,
    maxPayload: u32
}

static DEFAULT_ENGINE_CONFIG: EngineConfig = EngineConfig {
    pingInterval: 25000,
    pingTimeout: 20000,
    maxPayload: 1000000,
};

pub struct Engine<'a> {
    state:EngineState,
    buffer: VecDeque<ServerEvent>,
    config: &'a EngineConfig
}

impl <'a> Engine<'a> {
    pub fn new() -> Self { 
        return Self { state:EngineState::New, buffer: VecDeque::default(), config: &DEFAULT_ENGINE_CONFIG }
    }

    pub fn insert<T>(&mut self, input:T) -> Result<(),Error >
    where 
        T: TryInto<ClientEvent>,
        Error: From<T::Error>, 
    { 
        let engine_event = input.try_into() ?;

            match (&self.state, engine_event) { 
                ( EngineState::New , ClientEvent::Connect { eio, transport }) => self.connect(eio, transport),
                ( EngineState::New, _ ) => Err(Error::InvalidStateTransition),
                ( EngineState::Connected(current_transport), ClientEvent::Upgrade) =>  {
                    match current_transport {
                        EngineTransport::Polling => self.upgrade_to_socket(),
                        _ => Err(Error::InvalidStateTransition)
                    }
                }
                _ => Err(Error::UnknownInput)
            }
    }

    pub fn poll_output(&mut self) -> EngineOutput {
        return self.buffer.pop_front()
            .map(|e| EngineOutput::Event(e))
            .or(Some(EngineOutput::Wait))
            .unwrap();
    }

    // State Transitions 
    fn connect(& mut self, eio:u8, transport:EngineTransport) -> Result<(),Error> {
        if eio != 4 { return Err(Error::UnsupportedProtocolVersion) };

        let upgrades = match &transport { 
            EngineTransport::Socket => EngineTransport::None,
            EngineTransport::Polling => EngineTransport::Socket,
            EngineTransport::None => EngineTransport::Polling
        };

        self.state = EngineState::Connected(transport);

        self.buffer.push_back(
            ServerEvent::NewConnection { 
                sid:"".to_string(),
                upgrades: [upgrades],
                pingInterval: self.config.pingInterval,
                pingTimeout: self.config.pingTimeout,
                maxPayload: self.config.maxPayload 
            }
        );
        return Ok(());
    }

    fn upgrade_to_socket(& mut self ) -> Result<(),Error> { 
        return Ok(());
    }
}

// ==========================

type Header = [String;2];
type QueryParam = [String;2];

struct HTTPRequest {
    verb:String,
    host:String,
    path:String,
    query:[QueryParam;10],
    headers: [Header;10],
    body: String
}   


impl HTTPRequest { 

    pub fn get_sid<'a>(&'a self) -> Option<&'a str> {
        self.query.iter()
            .find(|&p| p[0] == "sid")
            .map(|p| p[1].as_str())
    }
}

// ==========================

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}
