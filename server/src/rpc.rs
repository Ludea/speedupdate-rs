use std::{
    fs,
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use base64::{engine::general_purpose, Engine as _};
use futures::prelude::*;
use http::header::{AUTHORIZATION, CONTENT_TYPE};
use http_body_util::BodyExt;
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use libspeedupdate::{
    metadata::{v1, CleanName},
    repository::{BuildOptions, CoderOptions, PackageBuilder},
    workspace::{UpdateOptions, Workspace},
    Repository,
};
use notify::{Config, RecursiveMode, Watcher};
use ring::{
    rand,
    signature::{EcdsaKeyPair, KeyPair},
};
use serde::{Deserialize, Serialize};
use speedupdaterpc::repo_server::{Repo, RepoServer};
use speedupdaterpc::{
    BuildInput, BuildOutput, CurrentVersion, Empty, FileToDelete, ListPackVerBin, Options, Package,
    Platforms, RepoStatus, RepoStatusOutput, RepositoryPath, RepositoryStatus, Version, Versions,
};
use tokio::select;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{
    codec::CompressionEncoding,
    service::{AxumBody, AxumRouter, Routes},
    Request, Response, Status,
};
use tonic_web::GrpcWebLayer;
use tower::{Layer, Service};
use tower_http::cors::{Any, CorsLayer};

pub mod speedupdaterpc {
    tonic::include_proto!("speedupdate");
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    email: String,
    exp: u64,
    scope: String,
}

type ResponseStatusStream = Pin<Box<dyn Stream<Item = Result<RepoStatusOutput, Status>> + Send>>;
type ResponseBuildStream = Pin<Box<dyn Stream<Item = Result<BuildOutput, Status>> + Send>>;

pub struct RemoteRepository {}

#[tonic::async_trait]
impl Repo for RemoteRepository {
    async fn init(&self, request: Request<RepositoryPath>) -> Result<Response<Empty>, Status> {
        let repository_path = request.into_inner().path;
        let mut repo = Repository::new(PathBuf::from(repository_path.clone()));
        let reply = Empty {};
        if let Err(err) = fs::create_dir_all(repository_path.clone()) {
            tracing::error!("{}", err);
            return Err(Status::internal(err.to_string()));
        } else {
            match repo.init() {
                Ok(_) => {
                    tracing::info!("Repository initialized at {repository_path}");
                    return Ok(Response::new(reply));
                }
                Err(err) => {
                    tracing::error!("{}", err);
                    return Err(Status::internal(err.to_string()));
                }
            }
        }
    }

    async fn is_init(&self, request: Request<RepositoryPath>) -> Result<Response<Empty>, Status> {
        let repository_path = request.into_inner().path;
        let package_file = repository_path + "/packages";
        let package_file_path = Path::new(&package_file);
        if package_file_path.exists() {
            let reply = Empty {};
            Ok(Response::new(reply))
        } else {
            Err(Status::internal("Repo not initilalized"))
        }
    }

    type StatusStream = ResponseStatusStream;

    async fn status(
        &self,
        request: Request<RepositoryStatus>,
    ) -> Result<Response<Self::StatusStream>, Status> {
        let inner = request.into_inner();
        let repo_request = inner.path.clone();
        let repo_watch = inner.path.clone();
        let platforms = inner.platforms;
        let options = inner
            .options
            .unwrap_or(Options { build_path: ".".to_string(), upload_path: ".".to_string() });

        let mut subfolders = Vec::new();

        for host in platforms.clone() {
            match Platforms::try_from(host) {
                Ok(Platforms::Win64) => subfolders.push("/win64"),
                Ok(Platforms::MacosX8664) => subfolders.push("/macos_x86_64"),
                Ok(Platforms::MacosArm64) => subfolders.push("/macos_arm64"),
                Ok(Platforms::Linux) => subfolders.push("/linux"),
                _ => {}
            }
        }

        let request_future = async move {
            let mut state = RepoStatusOutput { status: Vec::new() };
            for folder in subfolders.clone() {
                state.status.push(
                    match repo_state(repo_request.clone() + "/" + folder, options.clone()) {
                        Ok(local_state) => local_state,
                        Err(err) => return Err(Status::internal(err)),
                    },
                );
            }
            let (local_tx, mut local_rx) = mpsc::channel(1);
            let (tx, rx) = mpsc::channel(128);

            send_message(tx.clone(), state);

            let config = Config::default().with_poll_interval(Duration::from_secs(1));
            let mut watcher = notify::PollWatcher::new(
                move |res| match res {
                    Ok(_) => {
                        if let Err(err) = local_tx.blocking_send(res) {
                            tracing::error!("{:?}", err);
                        }
                    }
                    Err(err) => tracing::error!("{}", err),
                },
                config,
            )
            .unwrap();

            for folder in subfolders.clone() {
                if Path::new(&(repo_request.clone() + folder + "/current")).exists() {
                    watcher
                        .watch(
                            Path::new(&(repo_request.clone() + folder + "/current")),
                            RecursiveMode::NonRecursive,
                        )
                        .unwrap();
                }
                watcher
                    .watch(
                        Path::new(&(repo_request.clone() + folder + "/packages")),
                        RecursiveMode::NonRecursive,
                    )
                    .unwrap();
                watcher
                    .watch(
                        Path::new(&(repo_request.clone() + folder + "/versions")),
                        RecursiveMode::NonRecursive,
                    )
                    .unwrap();
                if Path::new(&(repo_request.clone() + folder + &options.build_path)).exists() {
                    watcher
                        .watch(
                            Path::new(&(repo_request.clone() + folder + "/.build")),
                            RecursiveMode::NonRecursive,
                        )
                        .unwrap();
                }
            }
            let mut repo_array = RepoStatusOutput { status: Vec::new() };
            //println!("client disconnect");

            tokio::task::spawn(async move {
                let _watcher = watcher;
                while let Some(Ok(_)) = local_rx.recv().await {
                    for folder in subfolders.clone() {
                        match repo_state(repo_watch.clone() + folder, options.clone()) {
                            Ok(new_state) => {
                                repo_array.status.push(new_state);
                            }
                            Err(err) => { Err(Status::internal(err)) }.unwrap(),
                        };
                    }
                    send_message(tx.clone(), repo_array.clone());
                    repo_array.status.clear();
                }
            });

            let output_stream = ReceiverStream::new(rx);
            Ok(Response::new(Box::pin(output_stream) as Self::StatusStream))
        };

        let cancellation_future = async move {
            tracing::info!("Stop streaming {} status", inner.path);
            Err(Status::cancelled("Request cancelled by client"))
        };

        with_cancellation_handler(request_future, cancellation_future).await
    }

    async fn get_current_version(
        &self,
        request: Request<RepositoryPath>,
    ) -> Result<Response<CurrentVersion>, Status> {
        let inner = request.into_inner();

        let repository_path = inner.path;
        let repo = Repository::new(PathBuf::from(repository_path));
        match repo.current_version() {
            Ok(version) => {
                let reply = CurrentVersion { version: version.version().to_string() };
                Ok(Response::new(reply))
            }
            Err(err) => Err(Status::internal(err.to_string())),
        }
    }

    async fn set_current_version(
        &self,
        request: Request<Version>,
    ) -> Result<Response<Empty>, Status> {
        let inner = request.into_inner();

        let repository_path = inner.path;
        let mut repo = Repository::new(PathBuf::from(repository_path.clone()));

        let version_string = CleanName::new(inner.version).unwrap();

        let reply = Empty {};
        match repo.set_current_version(&version_string) {
            Ok(_) => {
                tracing::info!(
                    "{} is now the current version for {}",
                    version_string,
                    repository_path
                );
                return Ok(Response::new(reply));
            }
            Err(err) => {
                tracing::error!("{}", err);
                return Err(Status::internal(err.to_string()));
            }
        }
    }

    async fn register_version(&self, request: Request<Version>) -> Result<Response<Empty>, Status> {
        let inner = request.into_inner();
        let repository_path = inner.path;
        let repo = Repository::new(PathBuf::from(repository_path.clone()));
        let version_string = match CleanName::new(inner.version) {
            Ok(ver) => ver,
            Err(err) => {
                tracing::error!(err);
                return Err(Status::internal(err.to_string()));
            }
        };

        let description = inner.description;
        let description = description.unwrap_or_default();
        let version = v1::Version { revision: version_string.clone(), description };
        let reply = Empty {};
        match repo.register_version(&version) {
            Ok(_) => {
                tracing::info!("version {} registered for {}", version_string, repository_path);
                return Ok(Response::new(reply));
            }
            Err(err) => {
                tracing::error!("{}", err);
                return Err(Status::internal(err.to_string()));
            }
        }
    }

    async fn unregister_version(
        &self,
        request: Request<Version>,
    ) -> Result<Response<Empty>, Status> {
        let inner = request.into_inner();
        let repository_path = inner.path;
        let repo = Repository::new(PathBuf::from(repository_path.clone()));
        let version_string = CleanName::new(inner.version).unwrap();
        let reply = Empty {};
        match repo.unregister_version(&version_string) {
            Ok(_) => {
                tracing::info!("version {} deleted for {}", version_string, repository_path);
                return Ok(Response::new(reply));
            }
            Err(err) => {
                tracing::error!("{}", err);
                Err(Status::internal(err.to_string()))
            }
        }
    }

    async fn register_package(&self, request: Request<Package>) -> Result<Response<Empty>, Status> {
        let inner = request.into_inner();
        let package_name = inner.name;
        let repository_path = inner.path;
        let package = package_name + ".metadata";
        let repo = Repository::new(PathBuf::from(repository_path));
        let reply = Empty {};
        match repo.register_package(package.as_str()) {
            Ok(_) => Ok(Response::new(reply)),
            Err(err) => Err(Status::internal(err.to_string())),
        }
    }

    async fn unregister_package(
        &self,
        request: Request<Package>,
    ) -> Result<Response<Empty>, Status> {
        let inner = request.into_inner();
        let package_name = inner.name;
        let repository_path = inner.path;
        let package = package_name + ".metadata";
        let repo = Repository::new(PathBuf::from(repository_path));
        let reply = Empty {};
        match repo.unregister_package(package.as_str()) {
            Ok(_) => Ok(Response::new(reply)),
            Err(err) => Err(Status::internal(err.to_string())),
        }
    }

    async fn versions(
        &self,
        request: Request<RepositoryPath>,
    ) -> Result<Response<ListPackVerBin>, Status> {
        let inner = request.into_inner();
        let repository_path = inner.path;
        let repo = Repository::new(PathBuf::from(repository_path));
        let mut list_versions: Vec<String> = Vec::new();
        match repo.versions() {
            Ok(ver) => {
                for val in ver.iter() {
                    list_versions.push(val.revision().to_string())
                }
                let reply = ListPackVerBin { ver_pack_bin: list_versions };
                Ok(Response::new(reply))
            }
            Err(err) => Err(Status::internal(err.to_string())),
        }
    }

    async fn packages(
        &self,
        request: Request<RepositoryPath>,
    ) -> Result<Response<ListPackVerBin>, Status> {
        let inner = request.into_inner();
        let repository_path = inner.path;
        let repo = Repository::new(PathBuf::from(repository_path));
        let mut list_packages: Vec<String> = Vec::new();
        match repo.packages() {
            Ok(pack) => {
                for val in pack.iter() {
                    list_packages.push(val.package_data_name().to_string())
                }
                let reply = ListPackVerBin { ver_pack_bin: list_packages };
                Ok(Response::new(reply))
            }
            Err(err) => Err(Status::internal(err.to_string())),
        }
    }

    async fn available_packages(
        &self,
        request: Request<RepositoryPath>,
    ) -> Result<Response<ListPackVerBin>, Status> {
        let inner = request.into_inner();
        let repository_path = inner.path;
        let build_path = inner.build_path;

        let build_path = build_path.unwrap_or(".build".to_string());

        let repo = Repository::new(PathBuf::from(repository_path));
        match repo.available_packages(build_path.to_string()) {
            Ok(pack) => {
                let reply = ListPackVerBin { ver_pack_bin: pack };
                Ok(Response::new(reply))
            }
            Err(err) => Err(Status::internal(err.to_string())),
        }
    }

    type BuildStream = ResponseBuildStream;

    async fn build(
        &self,
        request: Request<BuildInput>,
    ) -> Result<Response<Self::BuildStream>, Status> {
        let inner = request.into_inner();
        let repository_path = inner.path;
        let repository = Repository::new(PathBuf::from(repository_path));

        let source_version = match CleanName::new(inner.version) {
            Ok(ver) => ver,
            Err(err) => {
                return Err(Status::internal(err.to_string()));
            }
        };
        let source_directory = PathBuf::from(inner.source_directory);
        let build_directory = PathBuf::from(inner.build_directory.unwrap_or(".build".to_string()));
        let mut builder = PackageBuilder::new(build_directory, source_version, source_directory);
        if let Some(num_threads) = inner.num_threads {
            builder.set_num_threads(num_threads.try_into().unwrap());
        }
        let mut options = BuildOptions::default();
        if let Some(compressors) = Some(inner.compressors) {
            options.compressors = compressors
                .iter()
                .map(|compressor| CoderOptions::from_static_str(compressor).unwrap())
                .collect();
        }
        let (tx, rx) = mpsc::channel(128);

        if let Some(patchers) = Some(inner.patcher) {
            options.patchers =
                patchers.iter().map(|s| CoderOptions::from_static_str(s).unwrap()).collect();
        }
        /*        if let Some(from) = Some(inner.from) {
            let mut prev_version = CleanName::new("".to_string()).unwrap();
            let prev_directory = builder.build_directory.join(".from");
            match fs::create_dir_all(&prev_directory) {
                Ok(_) => {
                    prev_version = match CleanName::new(from.unwrap()) {
                        Ok(ver) => ver,
                        Err(err) => {
                            return Err(Status::internal(err.to_string()));
                        }
                    };
                }
                Err(err) => {
                    return Err(Status::internal(err.to_string()));
                }
            };
            let link = repository.link();
            let mut workspace = Workspace::open(&prev_directory).unwrap();
            let goal_version = Some(prev_version.clone());
            let mut update_stream = workspace.update(&link, goal_version, UpdateOptions::default());

            let state = match update_stream.next().await {
                Some(Ok(state)) => state,
                Some(Err(err)) => {
                    return Err(Status::internal(err.to_string()));
                }
                None => unreachable!(),
            };
            let state = state.borrow();

            let progress = state.histogram.progress();
            let res = update_stream.try_for_each(|_state| future::ready(Ok(()))).await;
            if let Err(err) = res {
                return Err(Status::internal(err.to_string()));
            }
            match workspace.remove_metadata() {
                Ok(_) => (),
                Err(err) => {
                    return Err(Status::internal(err.to_string()));
                }
            }
            builder.set_previous(prev_version, prev_directory);
        }*/

        let mut build_stream = builder.build();
        match build_stream.next().await {
            Some(Ok(state)) => state,
            Some(Err(err)) => {
                return Err(Status::internal(err.to_string()));
            }
            None => unreachable!(),
        };

        let res = build_stream.try_for_each(|_state| future::ready(Ok(()))).await;
        if let Err(err) = res {
            return Err(Status::internal(err.to_string()));
        }

        let reply = BuildOutput { downloaded_bytes_start: 0, downloaded_bytes_end: 0 };
        tokio::spawn(async move {
            if let Err(err) = tx.send(Result::<_, Status>::Ok(reply)).await {
                Err(Status::internal(err.to_string()))
            } else {
                Ok(())
            }
        });

        let output_stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(output_stream) as Self::BuildStream))
    }

    async fn delete_file(&self, request: Request<FileToDelete>) -> Result<Response<Empty>, Status> {
        let file = request.into_inner().file;
        if let Err(err) = fs::remove_file(".build/".to_owned() + &file) {
            return Err(Status::internal(err.to_string()));
        }
        if let Err(err) = fs::remove_file(".build/".to_owned() + &file + ".metadata") {
            return Err(Status::internal(err.to_string()));
        }
        let reply = Empty {};
        Ok(Response::new(reply))
    }

    async fn delete_repo(
        &self,
        request: Request<RepositoryPath>,
    ) -> Result<Response<Empty>, Status> {
        let repo = request.into_inner().path;
        if let Err(err) = fs::remove_dir_all(repo.clone()) {
            return Err(Status::internal(err.to_string()));
        }
        tracing::info!("{} repository deleted", repo);
        let reply = Empty {};
        Ok(Response::new(reply))
    }
}

fn repo_state(path: String, options: Options) -> Result<RepoStatus, String> {
    let repo = Repository::new(PathBuf::from(path.clone()));
    let mut list_versions: Vec<Versions> = Vec::new();
    match repo.versions() {
        Ok(value) => {
            for val in value.iter() {
                let new_version = Versions {
                    revision: val.revision().to_string(),
                    description: val.description().to_string(),
                };
                list_versions.push(new_version);
            }
        }
        Err(error) => return Err("Versions : ".to_owned() + &error.to_string()),
    }

    let current_version = match repo.current_version() {
        Ok(value) => value.version().to_string(),
        Err(_) => "-".to_string(),
    };

    let mut list_packages = Vec::new();

    let size = match repo.packages() {
        Ok(value) => {
            for val in value.iter() {
                list_packages.push(val.package_data_name().to_string());
            }
            value.iter().map(|p| p.size()).sum::<u64>()
        }
        Err(error) => return Err("Packages: ".to_owned() + &error.to_string()),
    };

    let available_packages = match repo.available_packages(options.build_path) {
        Ok(pack) => pack,
        Err(err) => return Err("Available packages: ".to_owned() + &err.to_string()),
    };

    let mut available_binaries = Vec::new();
    let temp_binaries_folder = format!("{}/{}", path, &options.upload_path);
    let binaries_folder = Path::new(&temp_binaries_folder);
    match fs::read_dir(binaries_folder) {
        Ok(dir) => {
            for entry in dir {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    available_binaries
                        .push(path.file_name().unwrap().to_str().unwrap().to_string());
                }
            }
        }
        Err(err) => return Err("Available binaries: ".to_owned() + &err.to_string()),
    }

    let state = RepoStatus {
        size,
        current_version,
        versions: list_versions,
        packages: list_packages,
        available_packages,
        available_binaries,
    };

    Ok(state)
}

fn send_message(
    tx: tokio::sync::mpsc::Sender<Result<RepoStatusOutput, Status>>,
    message: RepoStatusOutput,
) {
    tokio::spawn(async move {
        let _ = tx.send(Result::<_, Status>::Ok(message)).await;
    });
}

async fn with_cancellation_handler<FRequest, FCancellation>(
    request_future: FRequest,
    cancellation_future: FCancellation,
) -> Result<Response<ResponseStatusStream>, Status>
where
    FRequest: Future<Output = Result<Response<ResponseStatusStream>, Status>> + Send + 'static,
    FCancellation: Future<Output = Result<Response<ResponseStatusStream>, Status>> + Send + 'static,
{
    let token = CancellationToken::new();

    let _drop_guard = token.clone().drop_guard();
    let select_task = tokio::spawn(async move {
        select! {
            res = request_future => res,
            _ = token.cancelled() => cancellation_future.await,
        }
    });

    select_task.await.unwrap()
}

pub fn rpc_api() -> AxumRouter {
    let repo = RemoteRepository {};
    let service = RepoServer::new(repo)
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip);

    let mut routes = Routes::builder();
    routes.add_service(service);

    let cors_layer = CorsLayer::new()
        .allow_origin(Any)
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            http::header::HeaderName::from_static("x-grpc-web"),
            http::header::HeaderName::from_static("x-user-agent"),
        ])
        .expose_headers(Any);

    let layer = tower::ServiceBuilder::new().layer(AuthMiddlewareLayer::default()).into_inner();

    routes.routes().into_axum_router().layer(GrpcWebLayer::new()).layer(cors_layer).layer(layer)
}

#[derive(Debug, Clone, Default)]
pub struct AuthMiddlewareLayer {}

impl<S> Layer<S> for AuthMiddlewareLayer {
    type Service = AuthMiddleware<S>;

    fn layer(&self, service: S) -> Self::Service {
        AuthMiddleware { inner: service }
    }
}

#[derive(Debug, Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
}

type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

impl<S> Service<http::Request<AxumBody>> for AuthMiddleware<S>
where
    S: Service<http::Request<AxumBody>, Response = http::Response<AxumBody>>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<axum::body::Body>) -> Self::Future {
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            let (parts, body) = req.into_parts();
            let encoded_pkcs8 = fs::read_to_string("pkey").unwrap();
            let decoded_pkcs8 = general_purpose::STANDARD.decode(encoded_pkcs8).unwrap();
            let rng = &rand::SystemRandom::new();
            let pair = EcdsaKeyPair::from_pkcs8(
                &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
                &decoded_pkcs8,
                rng,
            )
            .unwrap();
            let decoding_key = &DecodingKey::from_ec_der(pair.public_key().as_ref());

            let content = body
                .collect()
                .await
                .map_err(|_err| {
                    println!("error");
                })
                .unwrap()
                .to_bytes();

            let content_vec = content.to_vec();
            let content_string = String::from_utf8(content_vec).unwrap();
            let content_without_ascii: Vec<_> =
                content_string.chars().filter(|&c| !(c as u32 > 0x001F)).collect();
            let content_string_without_ascii: String = content_without_ascii.into_iter().collect();
            let content_without_path = content_string_without_ascii
                .replace("/win64", "")
                .replace("/macos_arm64", "")
                .replace("/macos_x86_64", "")
                .replace("/linux", "");

            tracing::info!("content : {:?}", content_without_path);

            match parts.headers.get("authorization") {
                Some(t) => {
                    let validation = &mut Validation::new(Algorithm::ES256);
                    validation.validate_exp = false;
                    let t_string = t.to_str().unwrap().replace("Bearer ", "");
                    match decode::<Claims>(&t_string, decoding_key, validation) {
                        Ok(token_data) => {
                            // Compare body with scope
                            if token_data.claims.scope == content_without_path {
                                let body = AxumBody::from(content);
                                let response = inner
                                    .call(http::Request::from_parts(parts, body))
                                    .await
                                    .map_err(|_err| {
                                        println!("error");
                                    })
                                    .unwrap();
                                Ok(response)
                            } else {
                                Ok(Status::unauthenticated("Not allowed").into_http())
                            }
                        }
                        Err(err) => Ok(Status::unauthenticated(err.to_string()).into_http()),
                    }
                }
                None => Ok(Status::unauthenticated("No token found").into_http()),
            }
        })
    }
}
