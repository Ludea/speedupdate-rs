use libspeedupdate::{
    metadata::{v1, CleanName},
    repository::{BuildOptions, CoderOptions, PackageBuilder},
    workspace::{Workspace, UpdateOptions},
    Repository,
};
use futures::prelude::*;
use speedupdaterpc::repo_server::{Repo, RepoServer};
use speedupdaterpc::{
    BuildInput, BuildOutput, Package, RepositoryPath, ResponseResult, StatusResult, Version,
};
use std::{fs, io::ErrorKind, path::PathBuf};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};
use tonic_web::GrpcWebLayer;
use tower_http::cors::{Any, CorsLayer};

pub mod speedupdaterpc {
    tonic::include_proto!("speedupdate");
}

#[derive(Default)]
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
        match repo.init() {
            Ok(_) => (),
            Err(err) => reply = ResponseResult { error: err.to_string() },
        }
        Ok(Response::new(reply))
    }

    type StatusStream = ReceiverStream<Result<StatusResult, Status>>;

    async fn status(
        &self,
        request: Request<RepositoryPath>,
    ) -> Result<Response<Self::StatusStream>, Status> {
        let repository_path = request.into_inner().path;
        let repo = Repository::new(PathBuf::from(repository_path));
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

        let reply = StatusResult {
            repoinit,
	    size,
            current_version,
            versions: list_versions,
            packages: list_packages,
        };

        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            tx.send(Ok(reply)).await.unwrap();
        });

        Ok(Response::new(ReceiverStream::new(rx)))
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
            let state = match update_stream.next().await {
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
            builder.set_previous(prev_version, prev_directory);
        }

        let mut build_stream = builder.build();
        /*let mut build_state;
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

pub async fn start_rpc_server() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = "0.0.0.0:50051".parse().unwrap();

    let repository = RemoteRepository::default();
    let svc = RepoServer::new(repository);
    tracing::info!("SpeedupdateRPCServer listening on {}", addr);

    let cors_layer = CorsLayer::new().allow_origin(Any).allow_headers(Any).expose_headers(Any);

    Server::builder()
        .accept_http1(true)
        .layer(cors_layer)
        .layer(GrpcWebLayer::new())
	.add_service(svc)
        .serve(addr)
        .await?;

    Ok(())
}
