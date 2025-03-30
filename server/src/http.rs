use std::{
    convert::Infallible,
    fs,
    future::ready,
    io::{self, Read},
};

use axum::{
    extract::{DefaultBodyLimit, MatchedPath, Multipart, Path, Request},
    handler::HandlerWithoutStateExt,
    http::{header::CONTENT_LENGTH, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, Sse},
        IntoResponse,
    },
    routing::{get, get_service, on, post, MethodFilter},
    Router,
};
use futures::stream::Stream;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use tokio::time::{sleep, Duration};
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
use zip::result::ZipError;

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

pub fn http_api() -> Router {
    let (progress_tx, _) = broadcast::channel(100);

    let recorder_handle = setup_metrics_recorder();

    async fn handle_404() -> (StatusCode, &'static str) {
        (StatusCode::NOT_FOUND, "Not found")
    }

    let service = handle_404.into_service();
    let serve_dir = ServeDir::new(".").not_found_service(service);

    Router::new()
        .route("/health", get(health_check))
        .route("/metrics", get(move || ready(recorder_handle.render())))
        .route(
            "/{repo}/{type}/{folder}/{platform}",
            on(MethodFilter::POST, {
                let progress_tx = progress_tx.clone();
                move |header, path, multipart| {
                    save_binaries(progress_tx.clone(), header, path, multipart)
                }
            })
            .on(MethodFilter::GET, get_service(serve_dir)),
        )
        .route(
            "/{repo}/launcher",
            post({
                let progress_tx = progress_tx.clone();
                move |header, path, multipart| {
                    save_image(progress_tx.clone(), header, path, multipart)
                }
            }),
        )
        .route("/{repo}/{type}/progression", get(move || sse_handler(progress_tx)))
        .layer(DefaultBodyLimit::disable())
        .route_layer(middleware::from_fn(track_metrics))
        .layer(CorsLayer::new().allow_origin(Any).allow_headers(Any).expose_headers(Any))
        .layer(TraceLayer::new_for_http())
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

async fn save_binaries(
    progress_tx: Sender<(usize, usize)>,
    header: HeaderMap,
    Path((repo, launcher_game, folder, platform)): Path<(String, String, String, String)>,
    multipart: Multipart,
) -> Result<(), (StatusCode, String)> {
    let repo_path = std::path::Path::new(&repo);
    let folder_path = format!("{}/{}/{}/{}", repo.clone(), launcher_game, folder, platform);
    let upload_path = std::path::Path::new(&folder_path);

    upload(progress_tx, multipart, header, repo_path, upload_path).await?;

    Ok(())
}

async fn save_image(
    progress_tx: Sender<(usize, usize)>,
    header: HeaderMap,
    Path(repo): Path<String>,
    multipart: Multipart,
) -> Result<(), (StatusCode, String)> {
    let repo_path = std::path::Path::new(&repo);
    let upload_path = std::path::Path::new(&repo);

    upload(progress_tx, multipart, header, repo_path, upload_path).await?;

    Ok(())
}

async fn upload(
    progress_tx: Sender<(usize, usize)>,
    mut multipart: Multipart,
    header: HeaderMap,
    repo: &std::path::Path,
    upload_path: &std::path::Path,
) -> Result<(), (StatusCode, String)> {
    let content_length = header.get(CONTENT_LENGTH).unwrap().to_str().unwrap();
    let total_size = content_length.parse::<usize>().unwrap();
    let mut file_name = String::new();

    if repo.exists() && repo.is_dir() {
        if let Err(err) = fs::create_dir_all(upload_path.display().to_string()) {
            return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string()));
        }
        while let Some(mut field) = multipart
            .next_field()
            .await
            .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?
        {
            file_name = field.file_name().unwrap().to_string();
            let mut file =
                File::create(format!("{}/{}", &upload_path.display().to_string(), file_name))
                    .await
                    .unwrap();
            let mut progression = 0;
            while let Some(chunk) =
                field.chunk().await.map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?
            {
                progression += chunk.len();
                let _ = progress_tx.send((progression, total_size));
                file.write_all(&chunk).await.unwrap();
            }
            let _ = progress_tx.send((total_size, total_size));
        }

        tracing::info!(
            "File {} succesfully uploaded to {} folder",
            file_name,
            upload_path.display().to_string()
        );

        sleep(Duration::from_secs(2)).await;

        match is_zip_file(std::path::Path::new(&format!(
            "{}/{}",
            &upload_path.display(),
            file_name
        ))) {
            Ok(result) => {
                if result {
                    if let Err(err) =
                        extract_zip(format!("{}/{}", &upload_path.display(), file_name))
                    {
                        return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string()));
                    }
                    if let Err(err) =
                        fs::remove_file(format!("{}/{}", &upload_path.display(), file_name))
                    {
                        return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string()));
                    }
                }
            }
            Err(err) => {
                tracing::error!("{}", err);
                return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string()));
            }
        };
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

fn is_zip_file(file_path: &std::path::Path) -> io::Result<bool> {
    let mut file = std::fs::File::open(file_path)?;
    let mut signature = [0; 4];
    file.read_exact(&mut signature)?;
    Ok(signature == [0x50, 0x4B, 0x03, 0x04])
}

fn extract_zip(file_name: String) -> Result<(), ZipError> {
    let file = fs::File::open(&file_name).unwrap();

    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).unwrap();
        let file_enclosed_name = match file.enclosed_name() {
            Some(path) => path,
            None => continue,
        };

        {
            let comment = file.comment();
            if !comment.is_empty() {
                tracing::info!("File {i} comment: {comment}");
            }
        }

        let fullpath = std::path::Path::new(&file_name);
        if let Some(path_without_zip) = fullpath.parent() {
            let outpath = path_without_zip.join(file_enclosed_name);
            if file.is_dir() {
                tracing::info!("File {} extracted to \"{}\"", i, outpath.display());
                fs::create_dir_all(&outpath).unwrap();
            } else {
                tracing::info!(
                    "File {} extracted to \"{}\" ({} bytes)",
                    i,
                    outpath.display(),
                    file.size()
                );
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        fs::create_dir_all(p).unwrap();
                    }
                }
                let mut outfile = fs::File::create(&outpath).unwrap();
                io::copy(&mut file, &mut outfile).unwrap();
            }

            // Get and Set permissions
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                if let Some(mode) = file.unix_mode() {
                    fs::set_permissions(&outpath, fs::Permissions::from_mode(mode)).unwrap();
                }
            }
        }
    }
    Ok(())
}
