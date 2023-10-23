use actix_web::{middleware::Logger, App, HttpServer };
use engineio::adapters::actix::{ AsyncEngine, Emitter, socket_io };
use futures_util::StreamExt;

fn on_connection(mut stream:AsyncEngine, mut emit:Emitter) {
    actix_rt::spawn(async move {
       while let Some(v) = stream.next().await {
            emit.emit(v).await;
        }
    });
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    std::env::set_var("RUST_LOG", "debug");
    env_logger::init();

    HttpServer::new(|| 
           App::new()
           .wrap(Logger::default())
           .service(socket_io(actix_web::Resource::new("/sio"), on_connection))
        )
        .workers(1)
        .bind(("127.0.0.1", 8080))?
        .run()
        .await
}
