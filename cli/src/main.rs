use std::io::Write;
use std::path::{Path, PathBuf};
use std::{env, io, process};

use clap::{crate_authors, crate_description, crate_name, crate_version, Arg, ArgAction, Command};
use console::{style, Color};
use indicatif::WeakProgressBar;
use libspeedupdate::workspace::Workspace;
use libspeedupdate::Repository;
use log::{error, warn};
use parking_lot::RwLock;

mod repository;
mod workspace;

struct Logger {
    pb: RwLock<Option<WeakProgressBar>>,
    filter: RwLock<Option<env_filter::Filter>>,
}

impl Logger {
    const fn new() -> Self {
        Self { pb: parking_lot::const_rwlock(None), filter: parking_lot::const_rwlock(None) }
    }

    fn init(&self) {
        let filter = env_filter::Builder::from_env("RUST_LOG").build();
        log::set_max_level(filter.filter());
        *self.filter.write() = Some(filter);
    }

    fn set_progress_bar(&self, pb: Option<WeakProgressBar>) {
        let mut pb_guard = self.pb.write();
        *pb_guard = pb;
    }

    fn matches(&self, record: &log::Record) -> bool {
        match &*self.filter.read() {
            Some(filter) => filter.matches(record),
            None => record.level() <= log::max_level(),
        }
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        match &*self.filter.read() {
            Some(filter) => filter.enabled(metadata),
            None => metadata.level() <= log::max_level(),
        }
    }

    fn log(&self, record: &log::Record) {
        if !self.matches(record) {
            return;
        }

        let level = match record.level() {
            log::Level::Error => style("  ERROR  ").bg(Color::Red).black(),
            log::Level::Warn => style("  WARN   ").bg(Color::Red).black(),
            log::Level::Info => style("  INFO   ").bg(Color::Cyan).black(),
            log::Level::Debug => style("  DEBUG  ").bg(Color::Yellow).black(),
            log::Level::Trace => style("  TRACE  ").bg(Color::Magenta).black(),
        };
        let msg =
            format!("{} {}: {}", level, record.module_path().unwrap_or_default(), record.args());

        let pb = self.pb.read();
        let pb = pb.as_ref().and_then(|weak| weak.upgrade());
        match pb {
            Some(pb) => {
                pb.println(msg);
            }
            None => {
                writeln!(io::stderr(), "{}", msg).ok();
            }
        }
    }

    fn flush(&self) {
        io::stderr().flush().ok();
    }
}

static LOGGER: Logger = Logger::new();

#[tokio::main]
async fn main() {
    LOGGER.init();
    let _ = log::set_logger(&LOGGER);

    let matches = Command::new(crate_name!())
        .about(crate_description!())
        .author(crate_authors!("\n"))
        .version(crate_version!())
        .subcommand_required(true)
        .disable_help_subcommand(true)
        .arg_required_else_help(true)
        .arg(
            Arg::new("debug")
                .short('d')
                .value_parser(["warm", "info", "debug", "trace"])
                .default_value("info")
                .help("Sets the level of debugging information"),
        )
        .subcommand(
            Command::new("repository")
                .about("Manage repository")
                .arg_required_else_help(true)
                .arg(
                    Arg::new("local_repository")
                        .short('p')
                        .long("path")
                        .num_args(1)
                        .help("Repository path (defaults to current directory)"),
                )
                .subcommand(
                    Command::new("status")
                        .about("Show the repository status (current version & stats"),
                )
                .subcommand(Command::new("init").about("Initialize repository"))
                .subcommand(
                    Command::new("current_version").about("Show the repository current version"),
                )
                .subcommand(
                    Command::new("log")
                        .about("Show changelog")
                        .arg(Arg::new("from").num_args(1).help("from revision"))
                        .arg(Arg::new("to").num_args(1).help("to revision"))
                        .arg(Arg::new("online").help("Show one revision per line")),
                )
                .subcommand(
                    Command::new("packages")
                        .about("Show packages")
                        .arg(Arg::new("from").num_args(1).help("from revision"))
                        .arg(Arg::new("to").num_args(1).help("to revision")),
                )
                .subcommand(
                    Command::new("set_current_version")
                        .about("Set the repository current version")
                        .arg(Arg::new("version").num_args(1).required(true).help("Version to set")),
                )
                .subcommand(
                    Command::new("register_version")
                        .about("register_package or update version details")
                        .arg(
                            Arg::new("version")
                                .num_args(1)
                                .required(true)
                                .help("Version to add/update"),
                        )
                        .arg(Arg::new("description").num_args(1).help("Description string"))
                        .arg(Arg::new("description_file").num_args(1).help("Description string")),
                )
                .subcommand(Command::new("unregister_version").about("unregister version").arg(
                    Arg::new("version").num_args(1).required(true).help("Version to unregister"),
                ))
                .subcommand(
                    Command::new("register_package").about("Register or update package").arg(
                        Arg::new("package_metadata_name")
                            .num_args(1)
                            .required(true)
                            .help("Name of the package metadata file"),
                    ),
                )
                .subcommand(
                    Command::new("unregister_package").about("Unregister package").arg(
                        Arg::new("package_metadata_name")
                            .num_args(1)
                            .required(true)
                            .help("Name of the package metadata file"),
                    ),
                )
                .subcommand(
                    Command::new("build_package")
                        .about("Build package")
                        .arg(
                            Arg::new("version")
                                .num_args(1)
                                .required(true)
                                .help("Package output version"),
                        )
                        .arg(
                            Arg::new("source_dir")
                                .num_args(1)
                                .required(true)
                                .help("Source directory the package must represent"),
                        )
                        .arg(
                            Arg::new("from")
                                .long("from")
                                .num_args(1)
                                .help("Create a patch package from this revision"),
                        )
                        .arg(
                            Arg::new("register")
                                .long("register")
                                .action(ArgAction::SetTrue)
                                .help("Register the built package and its version"),
                        )
                        .arg(
                            Arg::new("compressor")
                                .long("compressor")
                                .num_args(1)
                                .help("Compressor options (i.e. \"brotli:6\")"),
                        )
                        .arg(
                            Arg::new("patcher")
                                .long("patcher")
                                .num_args(1)
                                .help("Patcher options (i.e. \"zstd:level=3;minsize=32MB\")"),
                        )
                        .arg(
                            Arg::new("num_threads")
                                .long("num-threads")
                                .num_args(1)
                                .help("Number of threads to use for building"),
                        )
                        .arg(
                            Arg::new("build_dir")
                                .long("build-dir")
                                .num_args(1)
                                .help("Directory where the build process will happen"),
                        )
                        .arg(
                            Arg::new("no_progress")
                                .long("no-progress")
                                .required(false)
                                .action(ArgAction::SetTrue)
                                .help("Disable progress bars"),
                        ),
                ),
        )
        .subcommand(
            Command::new("workspace")
                .about("Manage workspace")
                .arg_required_else_help(true)
                .arg(
                    Arg::new("workspace")
                        .short('p')
                        .long("path")
                        .num_args(1)
                        .help("Workspace directory"),
                )
                .subcommand(
                    Command::new("status")
                        .about("Show the workspace status")
                        .arg(Arg::new("repository").num_args(1).help("Repository URL")),
                )
                .subcommand(
                    Command::new("update")
                        .about("Update workspace")
                        .arg(Arg::new("repository").num_args(1).help("Repository URL"))
                        .arg(Arg::new("to").num_args(1).help("Target revision"))
                        .arg(
                            Arg::new("--check")
                                .help("Integrity check of all files, not just affected ones"),
                        )
                        .arg(Arg::new("no-progress").help("Disable progress bars")),
                )
                .subcommand(Command::new("check").about("Check workspace integrity"))
                .subcommand(
                    Command::new("log")
                        .about("Show changelog")
                        .arg(Arg::new("repository").num_args(1).help("Repository URL"))
                        .arg(Arg::new("--from").num_args(1).help("From revision"))
                        .arg(Arg::new("--to").num_args(1).help("Up to revision"))
                        .arg(
                            Arg::new("--latest").num_args(1).help("Use repository latest revision"),
                        )
                        .arg(Arg::new("--oneline").help("Show one revision per line")),
                ),
        )
        .get_matches();

    match matches.get_one::<String>("debug").map(String::as_str) {
        Some("warn") => log::set_max_level(log::LevelFilter::Warn),
        Some("info") => log::set_max_level(log::LevelFilter::Info),
        Some("debug") => log::set_max_level(log::LevelFilter::Debug),
        Some("trace") => log::set_max_level(log::LevelFilter::Trace),
        Some(lvl) => {
            warn!("invalid debug level '{}', ignoring...", lvl);
        }
        None => log::set_max_level(log::LevelFilter::Info),
    };

    match matches.subcommand() {
        Some(("repository", sub_matches)) => {
            let repository_path = match sub_matches.get_one::<String>("local_repository") {
                Some(path) => path.to_string(),
                None => std::env::current_dir().unwrap().display().to_string(),
            };
            eprintln!("repository: {}", repository_path);
            let mut repository = Repository::new(PathBuf::from(&repository_path));

            match sub_matches.subcommand() {
                Some(("status", sub_matches)) => {
                    repository::do_status(sub_matches, &mut repository).await
                }
                Some(("init", sub_matches)) => {
                    repository::do_init(sub_matches, &mut repository).await
                }
                Some(("current_version", sub_matches)) => {
                    repository::do_current_version(sub_matches, &mut repository).await
                }
                Some(("set_current_version", sub_matches)) => {
                    repository::do_set_current_version(sub_matches, &mut repository).await
                }
                Some(("log", sub_matches)) => {
                    repository::do_log(sub_matches, &mut repository).await
                }
                Some(("register_version", sub_matches)) => {
                    repository::do_register_version(sub_matches, &mut repository).await
                }
                Some(("unregister_version", sub_matches)) => {
                    repository::do_unregister_version(sub_matches, &mut repository).await
                }
                Some(("packages", sub_matches)) => {
                    repository::do_packages(sub_matches, &mut repository).await
                }
                Some(("register_package", sub_matches)) => {
                    repository::do_register_package(sub_matches, &mut repository).await
                }
                Some(("unregister_package", sub_matches)) => {
                    repository::do_unregister_package(sub_matches, &mut repository).await
                }
                Some(("build_package", sub_matches)) => {
                    repository::do_build_package(sub_matches, &mut repository).await
                }
                _ => unreachable!(),
            }
        }
        Some(("workspace", sub_matches)) => {
            let workspace_path = match sub_matches.get_one::<String>("workspace") {
                Some(path) => path.to_string(),
                None => std::env::current_dir().unwrap().display().to_string(),
            };
            println!("workspace: {}", workspace_path);
            let mut workspace = match Workspace::open(Path::new(&workspace_path)) {
                Ok(workspace) => workspace,
                Err(err) => {
                    error!("unable to load workspace state: {}", err);
                    process::exit(1)
                }
            };
            match sub_matches.subcommand() {
                Some(("status", sub_matches)) => {
                    workspace::do_status(sub_matches, &mut workspace).await
                }
                Some(("log", sub_matches)) => workspace::do_log(sub_matches, &mut workspace).await,
                Some(("check", sub_matches)) => {
                    workspace::do_check(sub_matches, &mut workspace).await
                }
                Some(("update", sub_matches)) => {
                    let repository = workspace::arg_repository(sub_matches).unwrap();
                    workspace::do_update(sub_matches, &mut workspace, &repository).await
                }
                _ => unreachable!(),
            };
        }
        _ => unreachable!(),
    };
}
