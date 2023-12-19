use actix_web::{middleware::Logger, App, HttpServer };
use engineio::adapters::actix::{ socket_io, AsyncEngineInner, NewConnectionService };
use futures_util::StreamExt;
use futures_util::Stream;
use futures_util::pin_mut;

struct NewConnectionManager {}
impl NewConnectionService for NewConnectionManager {
    fn new_connection<S:Stream + 'static>(&self, stream:S) {
        
        actix_rt::spawn(async move {
            pin_mut!(stream);
            while let Some(v) = stream.next().await {
                //emit.emit(v).await;
            }
        });
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    std::env::set_var("RUST_LOG", "debug");
    env_logger::init();

    HttpServer::new(|| 
           App::new()
           .wrap(Logger::default())
           .service(socket_io(actix_web::Resource::new("/sio"), NewConnectionManager {} ))
        )
        .workers(1)
        .bind(("127.0.0.1", 8080))?
        .run()
        .await
}
