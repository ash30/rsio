use reqwest;
use tokio::time::Instant;
use url::Url;
use crate::engine::{AsyncTransport, create_client_engine, Engine, EngineState};
use crate::io::create_session;
use crate::proto::{Payload, Sid};
use crate::io::Session;
use crate::engine::BaseEngine;

pub use crate::proto::TransportConfig;
use crate::transport::{TransportError, TransportKind};
use tokio::sync::mpsc;

pub fn io(url:impl reqwest::IntoUrl ) -> reqwest::Result<Session> {
    let client = reqwest::Client::new();
    let engine = create_client_engine(Instant::now().into());
    let url = url.into_url()?;
    let transport = ClientPollingTransport::new(url);
    let session = create_session(engine, transport);
    Ok(session)
}

struct ClientPollingTransport {
    url:Url,
    client:reqwest::Client,
    sid:Option<Sid>,
    config:Option<TransportConfig>,
    poll_rx: Option<mpsc::Receiver<Payload>>,
}

impl ClientPollingTransport {
    fn new(url:Url) -> Self {
        Self {
            url,
            client: reqwest::Client::new(),
            sid:None,
            config:None,
            poll_rx:None
        }
    }
}

impl AsyncTransport for ClientPollingTransport {
    async fn recv(&mut self) -> Result<Payload,crate::transport::TransportError> {
        // We should only start polling once connection 
        let Some(rx) = self.poll_rx.as_mut() else { return Err(TransportError::Generic)};
        let res = rx.recv().await;
        match res {
            Some(payload) => Ok(payload),
            None => Err(TransportError::Generic)
        }
    }

    async fn send(&mut self, data:Payload) -> Result<(),crate::transport::TransportError> {
        let Some(sid) = self.sid else { return Err(TransportError::Generic) };
        let mut url = self.url.clone();
        let query = format!("transport=polling&EIO=4&sid={}", sid);
        url.set_query(Some(&query));
        let data = data.encode(TransportKind::Poll);
        match self.client.post(url).body(data).send().await {
            Ok(_) => Ok(()),
            Err(e) => Err(TransportError::Generic)
        }
    }

    async fn engine_state_update(&mut self, next_state:crate::engine::EngineState) -> Result<Option<Vec<u8>>,TransportError>{
        match next_state {
            EngineState::Connecting(start) => {
                let mut url = self.url.clone();
                url.set_query(Some("transport=polling&EIO=4"));
                match self.client.get(url).send().await {
                    Err(e) => Err(TransportError::Generic),
                    Ok(r) => {
                        if let Ok(b) = r.bytes().await {
                          Ok(Some(b.into()))
                        }
                        else {
                            Err(TransportError::Generic)
                        }
                    }
                }
            },

            EngineState::Connected(_,config,sid) => {
                // ON first connect, setup task to poll session
                if self.poll_rx.is_none() {
                    self.sid = Some(sid);
                    let client = self.client.clone();
                    let mut url = self.url.clone();

                    let (tx,rx) = tokio::sync::mpsc::channel(1);
                    self.poll_rx = Some(rx);

                    // TODO:Should probably refactor to allow consuming code to poll
                    tokio::spawn(async move {
                        let mut sid_url = url.clone();
                        let query = format!("sid={}&transport=polling&EIO=4",sid);
                        sid_url.set_query(Some(&query));
                        while !tx.is_closed() {
                            let res = client.get(sid_url.clone())
                                .send().await
                                .map_err(|_|TransportError::Generic)?;

                            let b = res.bytes().await
                                .map_err(|_| TransportError::Generic)?;

                            let v = Payload::decode_combined(&b, TransportKind::Poll)
                                .map_err(|_|TransportError::Generic)?;

                            for p in v {
                                tx.send(p).await;
                            }
                        }
                        Ok::<(),TransportError>(())
                    });
                }
                Ok(None)
            }
            _ => Ok(None)
        }
    }
}


