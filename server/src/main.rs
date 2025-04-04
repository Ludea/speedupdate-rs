use std::net::SocketAddrV4;

use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

//mod ftp;
mod http;
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

    let addr: SocketAddrV4 = "0.0.0.0:8012".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    let grpc = rpc::rpc_api();
    let http = http::http_api();
    let app = Router::new()
        .merge(grpc)
        .merge(http)
        .layer(CorsLayer::new().allow_origin(Any).allow_headers(Any).expose_headers(Any));

    tracing::info!("Speedupdate gRPC and http server listening on {addr}");

    axum::serve(listener, app).await.unwrap();

    //let ftp_server = tokio::spawn(ftp::start_ftp_server());
}
