use axum::{
    extract::MatchedPath,
    http::Request,
    middleware::{self, Next},
    response::IntoResponse,
    routing::get,
    Router,
};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::{future::ready, net::SocketAddr};
use tower_http::services::ServeDir;

async fn health_check() -> &'static str {
    "OK"
}

fn metrics_app() -> Router {
    let recorder_handle = setup_metrics_recorder();
    Router::new().route("/metrics", get(move || ready(recorder_handle.render())))
}

async fn start_metrics_server() {
    let app = metrics_app();

    // NOTE: expose metrics enpoint on a different port
    let addr = SocketAddr::from(([0, 0, 0, 0], 3001));
    tracing::info!("HTTP metric server listening on {}", addr);
    axum::Server::bind(&addr).serve(app.into_make_service()).await.unwrap()
}

fn setup_metrics_recorder() -> PrometheusHandle {
    const EXPONENTIAL_SECONDS: &[f64] =
        &[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];

    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full("http_requests_duration_seconds".to_string()),
            EXPONENTIAL_SECONDS,
        )
        .unwrap()
        .install_recorder()
        .unwrap()
}

async fn start_main_server() {
    let app = main_app();

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("HTTP server listening on {}", addr);
    axum::Server::bind(&addr).serve(app.into_make_service()).await.unwrap()
}

fn main_app() -> Router {
    let serve_dir = ServeDir::new("/opt/speedupdate");

    Router::new()
        .nest_service("/", serve_dir.clone())
        .route_layer(middleware::from_fn(track_metrics))
	.route("/health", get(health_check))
}

async fn track_metrics<B>(req: Request<B>, next: Next<B>) -> impl IntoResponse {
    let path = if let Some(matched_path) = req.extensions().get::<MatchedPath>() {
        matched_path.as_str().to_owned()
    } else {
        req.uri().path().to_owned()
    };
    let method = req.method().clone();

    let response = next.run(req).await;

    let status = response.status().as_u16().to_string();

    let labels = [("method", method.to_string()), ("path", path), ("status", status)];

    metrics::counter!("http_requests_total", &labels).increment(1);

    response
}

pub async fn start_http_server() {
    // The `/metrics` endpoint should not be publicly available. If behind a reverse proxy, this
    // can be achieved by rejecting requests to `/metrics`. In this example, a second server is
    // started on another port to expose `/metrics`.
    let (_main_server, _metrics_server) = tokio::join!(start_main_server(), start_metrics_server());
}