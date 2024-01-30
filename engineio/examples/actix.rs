use actix_web::{middleware::Logger, App, HttpServer };
use engineio::{TransportConfig, MessageData};
use engineio::adapters::actix::{ socket_io, ConnectionService, Session};
use futures_util::StreamExt;
use futures_util::Stream;
use futures_util::pin_mut;

struct NewConnectionManager {}
impl ConnectionService for NewConnectionManager {
    fn new_connection<S:Stream<Item=Result<MessageData,engineio::EngineCloseReason>> +'static + Send>(&self, stream:S, emit:Session) {

        tokio::spawn(async move {
            pin_mut!(stream);
            loop {
                match stream.next().await {
                    None => break,
                    Some(Ok(data)) => { 
                        emit.send(data).await
                    },
                    _ => {}
                }
            }
        });
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    std::env::set_var("RUST_LOG", "debug");
    env_logger::init();

    let config = TransportConfig {
        ping_interval:300,
        ping_timeout:60200,
        ..TransportConfig::default()
    };

    HttpServer::new(move || 
           App::new()
           .wrap(Logger::default())
           .service(socket_io(actix_web::Resource::new("/engine.io/"), config, NewConnectionManager {} ))
        )
        .workers(1)
        .bind(("127.0.0.1", 3000))?
        .run()
        .await
}
