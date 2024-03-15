use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod ftp;
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

    let rpc_server = tokio::spawn(rpc::start_rpc_server());
    let ftp_server = tokio::spawn(ftp::start_ftp_server());
    let http_server = tokio::spawn(http::start_http_server());

    match tokio::try_join!(rpc_server, ftp_server, http_server) {
        Ok(_) => (),
        Err(err) => tracing::error!("Error {}", err),
    }
}
