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

    if let Err(err) = tokio::join!(rpc::rpc_api(), http::http_api()).0 {
        tracing::error!("Unable to start Speedupdate gRPC and HTTP server: {err}");
    }
    //let ftp_server = tokio::spawn(ftp::start_ftp_server());
}
