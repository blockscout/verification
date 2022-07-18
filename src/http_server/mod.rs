pub mod handlers;
mod metrics;
mod routers;

pub use self::routers::{configure_router, AppRouter, Router};

use crate::config::Config;
use actix_web::{App, HttpServer};

use futures::future;
use std::sync::Arc;

pub async fn run(config: Config) -> std::io::Result<()> {
    let socket_addr = config.server.addr;
    log::info!("Verification server is starting at {}", socket_addr);
    let app_router = Arc::new(
        AppRouter::new(config)
            .await
            .expect("couldn't initialize the app"),
    );
    let metrics = metrics::Metrics::new("/metrics".to_string());
    let metrics_future = metrics.run_private_server(6060);
    let server_future = {
        let metrics = metrics.public().clone();
        HttpServer::new(move || {
            App::new()
                .wrap(metrics.clone())
                .configure(configure_router(&*app_router))
        })
        .bind(socket_addr)?
        .run()
    };
    future::try_join(server_future, metrics_future).await?;
    Ok(())
}
