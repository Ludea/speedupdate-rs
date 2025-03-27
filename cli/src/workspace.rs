use std::ops::Deref;
use std::path::Path;
use std::process;

use clap::ArgMatches;
use console::{style, Term};
use futures::prelude::*;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use libspeedupdate::link::{AutoRepository, RemoteRepository};
use libspeedupdate::metadata::{self, v1::State, CleanName, Operation};
use libspeedupdate::workspace::{UpdateOptions, Workspace};
use log::error;

use crate::LOGGER;

pub fn arg_repository(matches: &ArgMatches) -> Option<AutoRepository> {
    match matches.get_one::<String>("repository") {
        Some(url) => {
            println!("repository: {}", url);
            match AutoRepository::new(url, None) {
                Ok(r) => Some(r),
                Err(err) => {
                    error!("{}", err);
                    process::exit(1)
                }
            }
        }
        None => None,
    }
}

async fn try_current_version(repository: &impl RemoteRepository) -> Option<metadata::Current> {
    match repository.current_version().await {
        Ok(current_version) => Some(current_version),
        Err(err) => {
            error!("unable to load repository current version: {}", err);
            None
        }
    }
}
async fn current_version(repository: &impl RemoteRepository) -> metadata::Current {
    match try_current_version(repository).await {
        Some(current_version) => current_version,
        None => std::process::exit(1),
    }
}

pub async fn do_status(matches: &ArgMatches, workspace: &mut Workspace) {
    let repository = arg_repository(matches);
    let current_version = match repository {
        Some(repository) => try_current_version(&repository).await,
        None => None,
    };
    match workspace.state() {
        State::New => {
            let latest = match current_version {
                Some(current_version) => format!(" (latest = {})", current_version.version()),
                None => String::new(),
            };
            let rev = style("NEW").bold();
            println!("status: {}{}", rev, latest);
        }
        State::Stable { version } => {
            let remote_status = match current_version {
                Some(current_version) if current_version.version() == version => {
                    style("UP to DATE").bold().green().to_string()
                }
                Some(current_version) => format!(
                    "{} (latest = {})",
                    style("OUTDATED").bold().dim(),
                    current_version.version()
                ),
                None => String::new(),
            };
            let rev = style(version).bold();
            println!("status: {}{}", rev, remote_status);
        }
        State::Corrupted { version, failures } => {
            let latest = match current_version {
                Some(current_version) => format!(" (latest = {})", current_version.version()),
                None => String::new(),
            };
            println!(
                "status: {rev} {version}{latest}",
                rev = style("CORRUPTED").bold().red(),
                version = version,
                latest = latest,
            );
            if !failures.is_empty() {
                println!("{} pending repair files:", failures.len());
                for f in failures {
                    println!(" - {path}", path = f);
                }
            }
        }
        State::Updating(d) => {
            let latest = match current_version {
                Some(current_version) => format!(" (latest = {})", current_version.version()),
                None => String::new(),
            };
            println!(
                "status: {rev} {from} → {to}{latest}",
                rev = style("UPDATING").bold().yellow(),
                from = match &d.from {
                    Some(rev) => rev,
                    None => "⊘",
                },
                to = d.to,
                latest = latest,
            );
            if !d.failures.is_empty() {
                println!("{} pending recovery files:", d.failures.len());
                for f in &d.failures {
                    println!(" - {path}", path = f);
                }
            }
        }
    }
}

pub async fn do_update(
    matches: &ArgMatches,
    workspace: &mut Workspace,
    repository: &impl RemoteRepository,
) {
    let goal_version = match matches.get_one::<&str>("to") {
        Some(to) => match CleanName::new(to.to_string()) {
            Ok(rev) => Some(rev),
            Err(_) => {
                error!("invalid target version: {} (must match [A-Za-Z0-9_.-]+)", to);
                std::process::exit(1)
            }
        },
        None => None,
    };
    let mut update_options = UpdateOptions::default();
    update_options.check = matches.get_flag("check");
    let mut stream = workspace.update(repository, goal_version, update_options);

    let state = match stream.next().await {
        Some(Ok(state)) => state,
        Some(Err(err)) => {
            error!("update failed: {}", err);
            std::process::exit(1)
        }
        None => {
            println!("UP to DATE");
            return;
        }
    };

    let state = state.borrow();
    let progress = state.histogram.progress();

    println!("Target revision: {}", state.target_revision);

    let res = if matches.get_flag("no_progress") {
        drop(state); // drop the Ref<_>

        stream.try_for_each(|_state| future::ready(Ok(()))).await
    } else {
        let draw_target = ProgressDrawTarget::term(Term::buffered_stdout(), 8);
        let m = MultiProgress::with_draw_target(draw_target);
        const DL_TPL: &str =
        "Download [{wide_bar:cyan/blue}] {bytes:>8}/{total_bytes:8} ({bytes_per_sec:>10}, {eta:4}) {msg:32}";
        const IN_TPL: &str =
        "Decode   [{wide_bar:cyan/blue}] {bytes:>8}/{total_bytes:8} ({bytes_per_sec:>10}, {eta:4}) {msg:32}";
        const OU_TPL: &str =
            "Install  [{wide_bar:cyan/blue}] {bytes:>8}/{total_bytes:8} ({bytes_per_sec:>10}      ) {msg:32}";
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

        let res = stream
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
    println!("UP to DATE");
}

fn op_file_name(op: Option<&dyn Operation>) -> String {
    op.and_then(|op| Path::new(op.path().deref()).file_name())
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

pub async fn do_log(matches: &ArgMatches, workspace: &mut Workspace) {
    let repository = arg_repository(matches).unwrap();
    let from: Option<&std::string::String> = matches.get_one::<String>("from");
    let to = match (matches.get_one::<String>("to"), matches.get_flag("latest")) {
        (None, false) => match workspace.state() {
            State::Stable { version } => version.to_string(),
            _ => current_version(&repository).await.version().to_string(),
        },
        (Some(to), _) => to.to_string(),
        (_, true) => current_version(&repository).await.version().to_string(),
    };
    let versions = match repository.versions().await {
        Ok(versions) => versions,
        Err(err) => {
            error!("unable to load repository current version: {}", err);
            std::process::exit(1)
        }
    };
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

pub async fn do_check(matches: &ArgMatches, workspace: &mut Workspace) {
    let mut stream = workspace.check();
    let state = match stream.next().await {
        Some(Ok(state)) => state,
        Some(Err(err)) => {
            error!("check failed: {}", err);
            std::process::exit(1)
        }
        None => {
            println!("CHECKED");
            return;
        }
    };

    let state = state.borrow();
    let progress = state.histogram.progress();

    let res = if matches.get_flag("no_progress") {
        drop(state); // drop the Ref<_>

        stream.try_for_each(|_state| future::ready(Ok(()))).await
    } else {
        let draw_target = ProgressDrawTarget::term(Term::buffered_stdout(), 8);
        let m = MultiProgress::with_draw_target(draw_target);
        const CHECK_TPL: &str =
        "Check    [{wide_bar:cyan/blue}] {bytes:>8}/{total_bytes:8} ({bytes_per_sec:>10}, {eta:4}) {msg:32}";
        let sty = ProgressStyle::default_bar().progress_chars("##-");

        let check_bytes = m.add(ProgressBar::new(state.check_bytes));
        check_bytes.set_style(sty.clone().template(CHECK_TPL).unwrap());
        check_bytes.set_position(progress.checked_bytes);
        check_bytes.reset_eta();

        LOGGER.set_progress_bar(Some(check_bytes.clone().downgrade()));

        drop(state); // drop the Ref<_>

        let res = stream
            .try_for_each(|state| {
                let state = state.borrow();
                let progress = state.histogram.progress();
                check_bytes.set_position(progress.checked_bytes);
                check_bytes.set_length(state.check_bytes);
                check_bytes.set_message(op_file_name(state.current_operation()));

                future::ready(Ok(()))
            })
            .await;

        check_bytes.finish();

        res
    };

    if let Err(err) = res {
        error!("check failed: {}", err);
        std::process::exit(1)
    }
    println!("CHECKED");
}
