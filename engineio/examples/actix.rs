use actix_web::{middleware::Logger, App, HttpServer };
use engineio::adapters::actix::{engine_io, IOEngine, TransportConfig};
use futures_util::StreamExt;
use futures_util::pin_mut;

fn on_connection(connection:IOEngine) {
    tokio::spawn(async move {
        pin_mut!(connection);
        loop {
            match connection.next().await {
                None => break,
                Some(data) => {
                    connection.send(data).await;
                }
            };
        }
    });
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    std::env::set_var("RUST_LOG", "debug");
    env_logger::init();

    let config = TransportConfig {
        ping_interval:3000,
        ping_timeout:2000,
        ..TransportConfig::default()
    };

    HttpServer::new(move || 
           App::new()
           .wrap(Logger::default())
           .service(engine_io(actix_web::Resource::new("/engine.io/"), config, on_connection ))
        )
        .workers(1)
        .bind(("127.0.0.1", 3000))?
        .run()
        .await
}
