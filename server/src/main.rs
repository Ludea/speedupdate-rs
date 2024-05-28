use self::multiplex_service::MultiplexService;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

//mod ftp;
mod http;
mod multiplex_service;
mod rpc;
//mod utils;

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "INFO".into()),
        ))
        .with(
            tracing_subscriber::fmt::layer()
                .pretty()
                .with_writer(std::io::stdout)
                .with_target(false)
                .with_ansi(true)
                .with_line_number(false)
                .with_file(false),
        )
        .init();

    let http = http::http_api().await;
    let grpc = rpc::rpc_api().into_service();
    let service = MultiplexService::new(http, grpc);

    let addr = "0.0.0.0:3000".parse().unwrap();
    tracing::info!("Speedupdate gRPC and HTTP server listening on {}", addr);
    axum::Server::bind(&addr).serve(tower::make::Shared::new(service)).await.unwrap();

    //let ftp_server = tokio::spawn(ftp::start_ftp_server());

    //match tokio::try_join!(rpc_server, ftp_server, http_server) {
    //    Ok(_) => (),
    //    Err(err) => tracing::error!("Error {}", err),
    //}
}
