use axum::{
    body::Bytes,
    extract::{MatchedPath, Multipart, Request},
    http::StatusCode,
    middleware::{self, Next},
    response::IntoResponse,
    routing::{get, post},
    BoxError, Router,
};
use futures::{Stream, TryStreamExt};
//use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::{fs, future::ready, io, path::Path};
use tokio::{fs::File, io::BufWriter};
use tokio_util::io::StreamReader;
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
    trace::TraceLayer,
};

const UPLOADS_DIRECTORY: &str = "uploads";

async fn health_check() -> &'static str {
    "OK"
}

/* fn setup_metrics_recorder() -> PrometheusHandle {
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
}*/

<<<<<<< HEAD
pub async fn http_api() {
    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    tracing::info!("HTTP listening on {local_addr}");

    let serve_dir = ServeDir::new("/home/ludea/allo");
    let recorder_handle = setup_metrics_recorder();

    if let Err(err) = tokio::fs::create_dir_all(UPLOADS_DIRECTORY).await {
        tracing::error!("failed to create `uploads` directory: {}", err);
    }

    let app = Router::new()
        .nest_service("/", serve_dir.clone())
        .route_layer(middleware::from_fn(track_metrics))
        .route("/health", get(health_check))
        //  .route("/metrics", get(move || ready(recorder_handle.render())))
        .route("/file/:file_name", post(accept_form))
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

async fn stream_to_file<S, E>(path: &str, stream: S) -> Result<(), (StatusCode, String)>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: Into<BoxError>,
{
    if !path_is_valid(path) {
        return Err((StatusCode::BAD_REQUEST, "Invalid path".to_owned()));
    }

    async {
        // Convert the stream into an `AsyncRead`.
        let body_with_io_error = stream.map_err(|err| io::Error::new(io::ErrorKind::Other, err));
        let body_reader = StreamReader::new(body_with_io_error);
        futures::pin_mut!(body_reader);

        if let Some(file_stem_os) = Path::new(&path).file_stem() {
            if let Some(file_stem_str) = file_stem_os.to_str() {
                let path = std::path::Path::new(UPLOADS_DIRECTORY).join(file_stem_str).join(path);
                if let Err(err) =
                    fs::create_dir_all(UPLOADS_DIRECTORY.to_owned() + "/" + file_stem_str)
                {
                    tracing::error!("Error when creating folder: {}", err);
                }
                let mut file = BufWriter::new(File::create(path).await.unwrap());

                if let Err(err) = tokio::io::copy(&mut body_reader, &mut file).await {
                    tracing::error!("Error when trying to copy file: {}", err);
                }
                Ok(())
            } else {
                tracing::error!("File name is not valid");
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        } else {
            tracing::error!("File don't have valid name");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
    .await
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

fn path_is_valid(path: &str) -> bool {
    let path = std::path::Path::new(path);
    let mut components = path.components().peekable();

    if let Some(first) = components.peek() {
        if !matches!(first, std::path::Component::Normal(_)) {
            return false;
        }
    }

    components.count() == 1
}

async fn accept_form(mut multipart: Multipart) -> Result<(), (StatusCode, String)> {
    while let Some(field) = multipart.next_field().await.unwrap() {
        let file_name = field.file_name().unwrap().to_string();

        stream_to_file(&file_name, field).await?
    }
    Ok(())
}
