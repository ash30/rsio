use reqwest;
use tokio::time::Instant;
use url::Url;
use crate::engine::{AsyncTransport, create_client_engine};
use crate::io::create_session;
use crate::proto::Payload;
use crate::io::Session;
use crate::engine::Engine;

pub use crate::proto::TransportConfig;


pub fn io(url:impl reqwest::IntoUrl ) -> reqwest::Result<Session> {
    let client = reqwest::Client::new();
    let engine = create_client_engine(Instant::now().into());
    let url = url.into_url()?;
    let transport = ClientPollingTransport { url, client };
    let session = create_session(engine, transport);
    Ok(session)
}

struct ClientPollingTransport {
    url:Url,
    client:reqwest::Client
}

impl AsyncTransport for ClientPollingTransport {
    async fn recv(&mut self) -> Result<Payload,crate::transport::TransportError> {
        todo!();
    }

    async fn send(&mut self, data:Payload) -> Result<(),crate::transport::TransportError> {
        todo!()
    }

    async fn engine_state_update(&mut self, next_state:crate::engine::EngineState) {
        todo!()
    }
}


