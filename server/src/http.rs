use axum::{
    extract::{MatchedPath, Multipart, Path, Request},
    http::{header::CONTENT_LENGTH, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, Sse},
        IntoResponse,
    },
    routing::{get, post},
    Router,
};
use futures::stream::Stream;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::{convert::Infallible, fs, future::ready, net::SocketAddr};
use tokio::{
    fs::File,
    io::AsyncWriteExt,
    sync::broadcast::{self, Sender},
};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
    trace::TraceLayer,
};

async fn health_check() -> &'static str {
    "OK"
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

pub async fn http_api() {
    let (progress_tx, _) = broadcast::channel(100);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    tracing::info!("HTTP listening on {local_addr}");

    let serve_dir = ServeDir::new(".");
    let recorder_handle = setup_metrics_recorder();

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/metrics", get(move || ready(recorder_handle.render())))
        .route(
            "/{repo}/{folder}/{platform}",
            post({
                let progress_tx = progress_tx.clone();
                move |header, path, multipart| {
                    save_request_body(progress_tx.clone(), header, path, multipart)
                }
            }),
        )
        .route("/{repo}/progression", get(move || sse_handler(progress_tx)))
        .fallback_service(serve_dir)
        .route_layer(middleware::from_fn(track_metrics))
        .layer(CorsLayer::new().allow_origin(Any).allow_headers(Any).expose_headers(Any))
        .layer(TraceLayer::new_for_http());

    axum::serve(listener, app).await.unwrap();
}

async fn track_metrics(req: Request, next: Next) -> impl IntoResponse {
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

async fn save_request_body(
    progress_tx: Sender<(usize, usize)>,
    header: HeaderMap,
    Path((repo, folder, platform)): Path<(String, String, String)>,
    mut multipart: Multipart,
) -> Result<(), (StatusCode, String)> {
    let request_path = std::path::Path::new(&repo);
    let folder_path = format!("{}/{}/{}", repo.clone(), folder.clone(), platform);
    let upload_path = std::path::Path::new(&folder_path);

    let content_length = header.get(CONTENT_LENGTH).unwrap().to_str().unwrap();
    let total_size = content_length.parse::<usize>().unwrap();

    if request_path.exists() && request_path.is_dir() {
        if !upload_path.exists() {
            if let Err(err) = fs::create_dir(folder_path.clone()) {
                return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string()));
            }
        }
        while let Some(mut field) = multipart
            .next_field()
            .await
            .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?
        {
            let file_name = field.file_name().unwrap().to_string();
            let mut file = File::create(format!("{}/{}", &folder_path, file_name)).await.unwrap();
            let mut progression = 0;
            while let Some(chunk) =
                field.chunk().await.map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?
            {
                progression += chunk.len();
                if let Err(err) = progress_tx.send((progression, total_size)) {
                    return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string()));
                }
                file.write_all(&chunk).await.unwrap();
            }
            if let Err(err) = progress_tx.send((total_size, total_size)) {
                return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string()));
            }
            tracing::info!("File {} succesfully uploaded for {} repository", file_name, repo);
        }
    } else {
        return Err((StatusCode::BAD_REQUEST, "No repository found".to_string()));
    }
    Ok(())
}

async fn sse_handler(
    progress_tx: Sender<(usize, usize)>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = progress_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(move |result| match result {
        Ok(bytes) => {
            let percent = bytes.0 * 100 / bytes.1;
            Some(Ok(Event::default().data(percent.to_string())))
        }
        Err(_) => None,
    });

    Sse::new(stream)
}
