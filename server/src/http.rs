use axum::{
    body::Bytes,
    extract::{MatchedPath, Multipart},
    http::{Request, StatusCode},
    middleware::{self, Next},
    response::IntoResponse,
    routing::{get, post},
    BoxError, Router,
};
use futures::{Stream, TryStreamExt};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::{fs, future::ready, io, net::SocketAddr, path::Path};
use tokio::{fs::File, io::BufWriter};
use tokio_util::io::StreamReader;
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
};

const UPLOADS_DIRECTORY: &str = "uploads";

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

pub async fn http_api() -> Router {
    let serve_dir = ServeDir::new("/opt/speedupdate");
    if let Err(err) = tokio::fs::create_dir_all(UPLOADS_DIRECTORY).await {
        tracing::error!("failed to create `uploads` directory: {}", err);
    }

    Router::new()
        .nest_service("/", serve_dir.clone())
        .route_layer(middleware::from_fn(track_metrics))
        .route("/health", get(health_check))
        .route("/file/:file_name", post(accept_form))
        .layer(CorsLayer::new().allow_origin(Any).allow_headers(Any).expose_headers(Any))
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
                fs::create_dir_all(UPLOADS_DIRECTORY.to_owned() + "/" + file_stem_str);
                let mut file = BufWriter::new(File::create(path).await.unwrap());

                // Copy the body into the file.
                tokio::io::copy(&mut body_reader, &mut file).await;
                Ok::<_, io::Error>(());
            } else {
                println!("Le nom du fichier n'est pas valide.");
            }
        } else {
            println!("Le fichier n'a pas de nom valide.");
        }
    }
    .await;
    Ok(())
    //    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
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
