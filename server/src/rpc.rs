use futures::prelude::*;
use libspeedupdate::{
    metadata::{v1, CleanName},
    repository::{BuildOptions, CoderOptions, PackageBuilder},
    workspace::{UpdateOptions, Workspace},
    Repository,
};
use notify::{Config, RecursiveMode, Watcher};
use speedupdaterpc::repo_server::{Repo, RepoServer};
use speedupdaterpc::{
    BuildInput, BuildOutput, Package, RepositoryPath, ResponseResult, StatusResult, Version,
};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    pin::Pin,
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{
    transport::{server::Router, Server},
    Request, Response, Status,
};
use tonic_web::GrpcWebLayer;
use tower::layer::util::Stack;
use tower_http::cors::{Any, CorsLayer};

pub mod speedupdaterpc {
    tonic::include_proto!("speedupdate");
}

type ResponseStream = Pin<Box<dyn Stream<Item = Result<StatusResult, Status>> + Send>>;

pub struct RemoteRepository {}

#[tonic::async_trait]
impl Repo for RemoteRepository {
    async fn init(
        &self,
        request: Request<RepositoryPath>,
    ) -> Result<Response<ResponseResult>, Status> {
        let repository_path = request.into_inner().path;
        let mut repo = Repository::new(PathBuf::from(repository_path));
        let mut reply = ResponseResult { error: "".to_string() };
        if let Err(err) = repo.init() {
            tracing::error!("{}", err);
            reply = ResponseResult { error: err.to_string() };
        }
        Ok(Response::new(reply))
    }

    type StatusStream = ResponseStream;

    async fn status(
        &self,
        request: Request<RepositoryPath>,
    ) -> Result<Response<Self::StatusStream>, Status> {
        let repository_path = request.into_inner().path;
        let state = repo_state(repository_path.clone());

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
        watcher.watch(Path::new("./current"), RecursiveMode::NonRecursive).unwrap();
        watcher.watch(Path::new("./packages"), RecursiveMode::NonRecursive).unwrap();
        watcher.watch(Path::new("./versions"), RecursiveMode::NonRecursive).unwrap();

        tokio::task::spawn(async move {
            while let Some(Ok(file)) = local_rx.recv().await {
                println!("Received file: {:?}", file);
                let new_state = repo_state("./".to_string());
                send_message(tx.clone(), new_state);
            }
        });

        let output_stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(output_stream) as Self::StatusStream))
    }

    async fn set_current_version(
        &self,
        request: Request<Version>,
    ) -> Result<Response<ResponseResult>, Status> {
        let inner = request.into_inner();

        let repository_path = inner.path;
        let mut repo = Repository::new(PathBuf::from(repository_path));

        let version_string = CleanName::new(inner.version).unwrap();

        let mut reply = ResponseResult { error: "".to_string() };
        match repo.set_current_version(&version_string) {
            Ok(_) => (),
            Err(value) => reply = ResponseResult { error: value.to_string() },
        }
        Ok(Response::new(reply))
    }

    async fn register_version(
        &self,
        request: Request<Version>,
    ) -> Result<Response<ResponseResult>, Status> {
        let inner = request.into_inner();
        let repository_path = inner.path;
        let repo = Repository::new(PathBuf::from(repository_path));
        let mut reply = ResponseResult { error: "".to_string() };
        let version_string = CleanName::new(inner.version).unwrap();

        let description: Option<String> = None; // = inner.description;
        let description_file: Option<String> = None; // = inner.description_file
        let description = match (description, description_file) {
            (None, None) => String::new(),
            (None, Some(descfile)) => String::new(), //{
            /*      match descfile {
                "-" => {
                    let mut desc = String::new();
                    std::io::stdin().read_to_string(&mut desc).map(|_| desc)
                }
                path => std::fs::read_to_string(path),
            }}*/
            (Some(desc), None) => desc,
            (Some(_), Some(_)) => "foo".to_string(),
        };
        let version = v1::Version { revision: version_string, description };
        match repo.register_version(&version) {
            Ok(_) => (),
            Err(value) => reply = ResponseResult { error: value.to_string() },
        }

        Ok(Response::new(reply))
    }
    async fn unregister_version(
        &self,
        request: Request<Version>,
    ) -> Result<Response<ResponseResult>, Status> {
        let inner = request.into_inner();
        let repository_path = inner.path;
        let repo = Repository::new(PathBuf::from(repository_path));
        let mut reply = ResponseResult { error: "".to_string() };
        let version_string = CleanName::new(inner.version).unwrap();
        match repo.unregister_version(&version_string) {
            Ok(_) => (),
            Err(value) => reply = ResponseResult { error: value.to_string() },
        }
        Ok(Response::new(reply))
    }

    async fn register_package(
        &self,
        request: Request<Package>,
    ) -> Result<Response<ResponseResult>, Status> {
        let inner = request.into_inner();
        let repository_path = inner.path;
        let package = inner.name;
        let repo = Repository::new(PathBuf::from(repository_path));
        let mut reply = ResponseResult { error: "".to_string() };
        match repo.register_package(package.as_str()) {
            Ok(_) => (),
            Err(value) => reply = ResponseResult { error: value.to_string() },
        }
        Ok(Response::new(reply))
    }

    async fn unregister_package(
        &self,
        request: Request<Package>,
    ) -> Result<Response<ResponseResult>, Status> {
        let inner = request.into_inner();
        let repository_path = inner.path;
        let package = inner.name;
        let repo = Repository::new(PathBuf::from(repository_path));
        let mut reply = ResponseResult { error: "".to_string() };
        match repo.unregister_package(package.as_str()) {
            Ok(_) => (),
            Err(value) => reply = ResponseResult { error: value.to_string() },
        }
        Ok(Response::new(reply))
    }

    async fn available_packages(
        &self,
        request: Request<RepositoryPath>,
    ) -> Result<Response<ResponseResult>, Status> {
        let mut reply = ResponseResult { error: "".to_string() };
        Ok(Response::new(reply))
    }

    async fn build_package(
        &self,
        request: Request<BuildInput>,
    ) -> Result<Response<BuildOutput>, Status> {
        let inner = request.into_inner();
        let repository_path = inner.path;
        let repository = Repository::new(PathBuf::from(repository_path));
        let mut reply = BuildOutput { error: "".to_string() };
        let mut source_version = None;
        match CleanName::new(inner.version) {
            Ok(value) => source_version = Some(value),
            Err(err) => reply = BuildOutput { error: err },
        }
        let source_directory = PathBuf::from(inner.source_directory);
        let build_directory = PathBuf::from(inner.build_directory.unwrap_or(".build".to_string()));
        let mut builder =
            PackageBuilder::new(build_directory, source_version.unwrap(), source_directory);
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
        if let Some(patchers) = Some(inner.patcher) {
            options.patchers =
                patchers.iter().map(|s| CoderOptions::from_static_str(s).unwrap()).collect();
        }
        if let Some(from) = Some(inner.from) {
            let mut prev_version = CleanName::new("".to_string()).unwrap();
            let prev_directory = builder.build_directory.join(".from");
            match fs::create_dir_all(&prev_directory) {
                Ok(_) => match CleanName::new(from.unwrap()) {
                    Ok(value) => prev_version = value,
                    Err(err) => reply = BuildOutput { error: err },
                },
                Err(err) => reply = BuildOutput { error: err.to_string() },
            };
            let link = repository.link();
            let mut workspace = Workspace::open(&prev_directory).unwrap();
            let goal_version = Some(prev_version.clone());
            let mut update_stream = workspace.update(&link, goal_version, UpdateOptions::default());
            /*            let state = match update_stream.next().await {
                Some(Ok(state)) => {
                    reply = BuildOutput { error: "foo".to_string() };
                    state
                }
                Some(Err(err)) => {
                    reply = BuildOutput { error: "Update failed: ".to_string() + &err.to_string() };
                    std::process::exit(1)
                }
                None => unreachable!(),
            };
            let state = state.lock();
            let progress = state.histogram.progress();
            let res = update_stream.try_for_each(|_state| future::ready(Ok(()))).await;
            if let Err(err) = res {
                reply = BuildOutput { error: err.to_string() }
            }
            match workspace.remove_metadata() {
                Ok(_) => (),
                Err(error) => reply = BuildOutput { error: error.to_string() },
            }
            builder.set_previous(prev_version, prev_directory); */
        }

        /*let mut build_stream = builder.build();
        let mut build_state;
        let state = match build_stream.next().await {
            Some(Ok(state)) => state,
            Some(Err(err)) => {
                reply = BuildOutput { error: "build failed".to_string() + &err.to_string() };
                std::process::exit(1)
            }
            None => unreachable!(),
        };
        let state = state.borrow();
        let res = build_stream.try_for_each(|_state| future::ready(Ok(()))).await;
        if let Err(err) = res {
            reply = BuildOutput { error: err.to_string() }
        }*/
        Ok(Response::new(reply))
    }
}

fn repo_state(path: String) -> StatusResult {
    let repo = Repository::new(PathBuf::from(path));
    let current_version;
    let mut repoinit = false;
    match repo.current_version() {
        Ok(value) => current_version = value.version().to_string(),
        Err(error) => {
            if error.kind() == ErrorKind::NotFound {
                repoinit = false;
            }
            current_version = "-".to_string();
        }
    }
    let mut list_versions: Vec<String> = Vec::new();
    match repo.versions() {
        Ok(value) => {
            for val in value.iter() {
                list_versions.push(val.revision().to_string())
            }
            repoinit = true;
        }
        Err(error) => {
            if error.kind() == ErrorKind::NotFound {
                repoinit = false;
            }
        }
    }
    let mut list_packages = Vec::new();
    let mut size = 0;
    match repo.packages() {
        Ok(value) => {
            for val in value.iter() {
                list_packages.push(val.package_data_name().to_string());
            }
            size = value.iter().map(|p| p.size()).sum::<u64>();
        }
        Err(error) => {
            if error.kind() == ErrorKind::NotFound {
                repoinit = false;
            }
        }
    };

    let available_packages = repo.available_packages(".build".to_string()).unwrap();

    let mut available_binaries = Vec::new();
    let binaries_folder = Path::new("uploads");
    for entry in fs::read_dir(binaries_folder).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            available_binaries.push(path.file_name().unwrap().to_str().unwrap().to_string());
        }
    }

    let reply = StatusResult {
        repoinit: repoinit,
        size: size,
        current_version: current_version,
        versions: list_versions,
        packages: list_packages,
        available_packages: available_packages,
        available_binaries: available_binaries,
    };

    return reply;
}

fn send_message(
    tx: tokio::sync::mpsc::Sender<Result<StatusResult, Status>>,
    message: StatusResult,
) {
    tokio::spawn(async move {
        if let Err(err) = tx.send(Result::<_, Status>::Ok(message)).await {
            tracing::error!("{}", err);
        }
    });
}

pub fn rpc_api() -> Router<Stack<GrpcWebLayer, Stack<CorsLayer, tower::layer::util::Identity>>> {
    let repo = RemoteRepository {};
    let svc = RepoServer::new(repo);

    let cors_layer = CorsLayer::new().allow_origin(Any).allow_headers(Any).expose_headers(Any);

    Server::builder()
        .accept_http1(true)
        .layer(cors_layer)
        .layer(GrpcWebLayer::new())
        .add_service(svc)
}
