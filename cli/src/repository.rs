use std::fmt::Display;
use std::fs;
use std::io::Read;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use byte_unit::Byte;
use clap::ArgMatches;
use console::{style, Term};
use futures::prelude::*;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use libspeedupdate::metadata::{self, CleanName, Operation};
use libspeedupdate::repository::{BuildOptions, CoderOptions, PackageBuilder};
use libspeedupdate::workspace::{UpdateOptions, Workspace};
use libspeedupdate::Repository;
use log::{error, info};

use crate::LOGGER;

fn some_<T>(res: Option<T>, ctx: &str) -> T {
    match res {
        Some(value) => value,
        None => {
            error!("{}", ctx);
            std::process::exit(1);
        }
    }
}

fn try_<T, E: Display>(res: Result<T, E>, ctx: &str) -> T {
    match res {
        Ok(value) => value,
        Err(err) => {
            error!("unable to {}: {}", ctx, err);
            std::process::exit(1);
        }
    }
}

fn current_version(repository: &mut Repository) -> metadata::Current {
    try_(repository.current_version(), "load repository current version")
}

pub async fn do_status(_matches: &ArgMatches, repository: &mut Repository) {
    let current_version = current_version(repository);
    let versions = try_(repository.versions(), "load repository versions");
    let packages = try_(repository.packages(), "load repository versions");
    println!("current_version: {}", current_version.version());
    println!("versions: {}", versions.iter().count());
    println!("packages: {}", packages.iter().count());
    let size = Byte::from_u64(packages.iter().map(|p| p.size()).sum::<u64>());
    println!("size: {}", size);
}

pub async fn do_init(_matches: &ArgMatches, repository: &mut Repository) {
    try_(repository.init(), "initialize repository");
    println!("repository initialized !");
}

pub async fn do_set_current_version(matches: &ArgMatches, repository: &mut Repository) {
    let version: &_ = some_(matches.get_one::<String>("version"), "no version provided");
    let version = try_(
        CleanName::new(version.to_string()),
        "convert version to clean name (i.e. [A-Za-Z0-9_.-]+)",
    );
    try_(repository.set_current_version(&version), "set current version");
}

pub async fn do_current_version(_matches: &ArgMatches, repository: &mut Repository) {
    let current_version = current_version(repository);
    println!("{}", current_version.version());
}

pub async fn do_register_version(matches: &ArgMatches, repository: &mut Repository) {
    let version: &_ = some_(matches.get_one::<String>("version"), "no version provided");
    let version = try_(
        CleanName::new(version.to_string()),
        "convert version to clean name (i.e. [A-Za-Z0-9_.-]+)",
    );
    let description = match (
        matches.get_one::<String>("description"),
        matches.get_one::<String>("description_file"),
    ) {
        (None, None) => String::new(),
        (None, Some(descfile)) => try_(
            match descfile.as_ref() {
                "-" => {
                    let mut desc = String::new();
                    std::io::stdin().read_to_string(&mut desc).map(|_| desc)
                }
                path => std::fs::read_to_string(path),
            },
            "read description file",
        ),
        (Some(desc), None) => desc.to_string(),
        (Some(_), Some(_)) => {
            error!("--desc and --descfile are mutually exclusive");
            std::process::exit(1);
        }
    };
    let version = metadata::v1::Version { revision: version, description };
    try_(repository.register_version(&version), "register version");
}

pub async fn do_unregister_version(matches: &ArgMatches, repository: &mut Repository) {
    let version: &_ = some_(matches.get_one::<String>("version"), "no version provided");
    let version = try_(
        CleanName::new(version.to_string()),
        "convert version to clean name (i.e. [A-Za-Z0-9_.-]+)",
    );
    try_(repository.unregister_version(&version), "unregister version");
}

pub async fn do_packages(_matches: &ArgMatches, repository: &mut Repository) {
    let packages = try_(repository.packages(), "load repository packages");
    println!("packages: {}", packages.iter().count());
}

pub async fn do_register_package(matches: &ArgMatches, repository: &mut Repository) {
    let package_metadata_name = some_(
        matches.get_one::<String>("package_metadata_name"),
        "no package metadata file name provided",
    );
    try_(repository.register_package(package_metadata_name), "register package");
}

pub async fn do_unregister_package(matches: &ArgMatches, repository: &mut Repository) {
    let package_metadata_name = some_(
        matches.get_one::<String>("package_metadata_name"),
        "no package metadata file name provided",
    );
    try_(repository.unregister_package(package_metadata_name), "unregister package");
}

pub async fn do_log(matches: &ArgMatches, repository: &mut Repository) {
    let from = matches.get_one::<String>("from");
    let to: String = match matches.get_one::<String>("to") {
        None => current_version(repository).version().to_string(),
        Some(to) => to.to_string(),
    };
    let versions = try_(repository.versions(), "load repository versions");
    let skip_n = match from {
        Some(from) => match versions.iter().position(|v| v.revision().deref() == from) {
            Some(pos) => pos,
            None => {
                error!("unable to find starting version: {}", from);
                std::process::exit(1)
            }
        },
        None => 0,
    };
    let oneline = matches.get_flag("oneline");
    for version in versions.iter().skip(skip_n) {
        if oneline {
            println!(
                "{}: {}",
                style(&version.revision()).bold(),
                version.description().lines().next().unwrap_or("")
            );
        } else {
            println!("{}", style(&version.revision()).bold());
            if !version.description().is_empty() {
                println!();
                println!("{}", version.description());
                println!();
            }
        }
        if version.revision().deref() == to {
            break;
        }
    }
}

fn op_file_name(op: Option<&dyn Operation>) -> String {
    op.and_then(|op| Path::new(op.path().deref()).file_name())
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

pub async fn do_build_package(matches: &ArgMatches, repository: &mut Repository) {
    let source_version = some_(matches.get_one::<String>("version"), "no version provided");
    let source_version = try_(
        CleanName::new(source_version.to_string()),
        "convert version to clean name (i.e. [A-Za-Z0-9_.-]+)",
    );
    let source_directory =
        PathBuf::from(some_(matches.get_one::<String>("source_dir"), "no source dir provided"));
    let build_directory = match matches.get_one::<String>("build_dir") {
        Some(build_directory) => PathBuf::from(build_directory),
        None => repository.dir().join(".build"),
    };
    let mut builder = PackageBuilder::new(build_directory, source_version, source_directory);
    if let Some(num_threads) = matches.get_one::<String>("num_threads") {
        let num_threads =
            try_(usize::from_str_radix(num_threads, 1), "convert --num-threads to integer");
        builder.set_num_threads(num_threads);
    }
    let mut options = BuildOptions::default();
    if let Some(compressors) = matches.get_many::<String>("compressor") {
        options.compressors = compressors
            .map(|s| try_(CoderOptions::from_static_str(s), "load compressor options"))
            .collect();
    }
    if let Some(patchers) = matches.get_many::<String>("patcher") {
        options.patchers = patchers
            .map(|s| try_(CoderOptions::from_static_str(s), "load patcher options"))
            .collect();
    }
    if let Some(from) = matches.get_one::<String>("from") {
        let prev_directory = builder.build_directory.join(".from");
        try_(fs::create_dir_all(&prev_directory), "create from directory");
        let prev_version = try_(
            CleanName::new(from.to_string()),
            "convert from version to clean name (i.e. [A-Za-Z0-9_.-]+)",
        );

        let link = repository.link();
        let mut workspace = Workspace::open(&prev_directory).unwrap();
        let goal_version = Some(prev_version.clone());
        let mut update_stream = workspace.update(&link, goal_version, UpdateOptions::default());

        let state = match update_stream.next().await {
            Some(Ok(state)) => state,
            Some(Err(err)) => {
                error!("update failed: {}", err);
                std::process::exit(1)
            }
            None => unreachable!(),
        };

        let state = state.borrow();
        let progress = state.histogram.progress();

        let res = if matches.get_flag("no_progress") {
            update_stream.try_for_each(|_state| future::ready(Ok(()))).await
        } else {
            let draw_target = ProgressDrawTarget::term(Term::buffered_stdout(), 8);
            let m = MultiProgress::with_draw_target(draw_target);
            const DL_TPL: &str =
            "Download [{elapsed_precise}] {wide_bar:40.cyan/blue} {bytes:>8}/{total_bytes:8} ({bytes_per_sec:>10}, {eta:4}) {msg:32}";
            const IN_TPL: &str =
            "Decode   [{elapsed_precise}] {wide_bar:40.cyan/blue} {bytes:>8}/{total_bytes:8} ({bytes_per_sec:>10}, {eta:4}) {msg:32}";
            const OU_TPL: &str =
                "Install  [{elapsed_precise}] {wide_bar:40.cyan/blue} {bytes:>8}/{total_bytes:8} ({bytes_per_sec:>10}      ) {msg:32}";
            let sty = ProgressStyle::default_bar().progress_chars("##-");

            let dl_bytes = m.add(ProgressBar::new(state.download_bytes));
            dl_bytes.set_style(sty.clone().template(DL_TPL).unwrap());
            dl_bytes.set_position(progress.downloaded_bytes);
            dl_bytes.reset_eta();

            let apply_input_bytes = m.add(ProgressBar::new(state.apply_input_bytes));
            apply_input_bytes.set_style(sty.clone().template(IN_TPL).unwrap());
            apply_input_bytes.set_position(progress.applied_input_bytes);
            apply_input_bytes.reset_eta();

            let apply_output_bytes = m.add(ProgressBar::new(state.apply_output_bytes));
            apply_output_bytes.set_style(sty.clone().template(OU_TPL).unwrap());
            apply_output_bytes.set_position(progress.applied_output_bytes);
            apply_output_bytes.reset_eta();

            LOGGER.set_progress_bar(Some(dl_bytes.clone().downgrade()));

            drop(state); // drop the Ref<_>

            let res = update_stream
                .try_for_each(|state| {
                    let state = state.borrow();
                    let progress = state.histogram.progress();
                    dl_bytes.set_position(progress.downloaded_bytes);
                    dl_bytes.set_length(state.download_bytes);
                    dl_bytes.set_message(op_file_name(
                        state.current_step_operation(state.downloading_operation_idx),
                    ));

                    apply_input_bytes.set_position(progress.applied_input_bytes);
                    apply_input_bytes.set_length(state.apply_input_bytes);
                    apply_input_bytes.set_message(op_file_name(
                        state.current_step_operation(state.applying_operation_idx),
                    ));

                    apply_output_bytes.set_position(progress.applied_output_bytes);
                    apply_output_bytes.set_length(state.apply_output_bytes);
                    apply_output_bytes.set_message(format!("{:?}", state.stage));

                    future::ready(Ok(()))
                })
                .await;

            dl_bytes.finish();
            apply_input_bytes.finish();
            apply_output_bytes.finish();

            res
        };

        if let Err(err) = res {
            error!("update failed: {}", err);
            std::process::exit(1)
        }
        try_(workspace.remove_metadata(), "remove update metadata");
        builder.set_previous(prev_version, prev_directory);
    }

    let mut build_stream = builder.build();

    let state = match build_stream.next().await {
        Some(Ok(state)) => state,
        Some(Err(err)) => {
            error!("build failed: {}", err);
            std::process::exit(1)
        }
        None => unreachable!(),
    };

    let res = if matches.get_flag("no_progress") {
        build_stream.try_for_each(|_state| future::ready(Ok(()))).await
    } else {
        let draw_target = ProgressDrawTarget::term(Term::buffered_stdout(), 8);
        let m = MultiProgress::with_draw_target(draw_target);
        let sty = ProgressStyle::default_bar().progress_chars("##-");
        const TPL: &str =
        "[{wide_bar:0.cyan/blue}] {bytes:>8}/{total_bytes:8} ({bytes_per_sec:>10}, {eta:4}) {msg:32}";

        let mut bars = state
            .lock()
            .workers
            .iter()
            .enumerate()
            .map(|(idx, worker)| {
                let pb = m.add(ProgressBar::new(worker.process_bytes));
                pb.set_style(sty.clone().template(&format!("{}{}", idx, TPL)).unwrap());
                pb.set_position(worker.processed_bytes);
                pb.reset_eta();
                pb
            })
            .collect::<Vec<_>>();

        LOGGER.set_progress_bar(bars.first().map(|b| b.downgrade()));

        drop(state); // drop the Ref<_>

        let res = build_stream
            .try_for_each(|state| {
                let state = state.lock();
                for (worker, bar) in state.workers.iter().zip(bars.iter_mut()) {
                    bar.set_position(worker.processed_bytes);
                    bar.set_length(worker.process_bytes);
                    bar.set_message(worker.task_name.to_string());
                }

                future::ready(Ok(()))
            })
            .await;

        for bar in bars {
            bar.finish();
        }

        res
    };

    if let Err(err) = res {
        error!("build failed: {}", err);
        std::process::exit(1)
    }

    info!("Package `{}` built", builder.package_metadata_name());

    if matches.get_flag("register") {
        try_(builder.add_to_repository(repository), "register package");
    }
}
