//! kube-state-metrics in Rust (kube-state-met.rs): listens to the Kubernetes
//! API server and generates metrics about the state of the objects.
//! Go entry points: `main.go`, `internal/wrapper.go`.

mod auth;
mod builder;
mod client;
mod conn;
mod crs;
mod discovery;
mod handler;
mod logging;
mod options;
mod pgzip;
mod process;
mod reload;
mod server;
mod telemetry;
mod watch;
mod watch_dynamic;
mod web;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::options::Command;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match options::parse(&args) {
        Ok(Command::Help) => {
            print!("{}", options::help_text());
            return ExitCode::SUCCESS;
        }
        Ok(Command::Version) => {
            println!(
                "{}",
                options::version_text(
                    client::VERSION,
                    server::revision(),
                    option_env!("KSM_BRANCH").unwrap_or("")
                )
            );
            return ExitCode::SUCCESS;
        }
        Ok(Command::Completion(shell)) => {
            print!("{}", options::completion_text(&shell));
            return ExitCode::SUCCESS;
        }
        Ok(Command::VersionHelp) => {
            print!("{}", options::version_help_text());
            return ExitCode::SUCCESS;
        }
        Ok(Command::HelpCommandHelp) => {
            print!("{}", options::help_command_help_text());
            return ExitCode::SUCCESS;
        }
        Ok(Command::CompletionHelp) => {
            print!(
                "{}",
                options::completion_help_text(std::env::var("SHELL").ok().as_deref())
            );
            return ExitCode::SUCCESS;
        }
        Ok(Command::Run(opts)) => opts,
        Err(e) => {
            logging::init(0);
            logging::error_go("main.go:36", "Running kube-state-metrics failed", &e);
            return ExitCode::FAILURE;
        }
    };
    logging::init(opts.verbosity);
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            logging::error_go(
                "main.go:36",
                "Running kube-state-metrics failed",
                &e.to_string(),
            );
            return ExitCode::FAILURE;
        }
    };
    rt.block_on(run_wrapper(opts))
}

/// `RunKubeStateMetricsWrapper`: checks the config files, runs
/// kube-state-metrics and restarts it whenever the options config file, the
/// custom resource state config file or the kubeconfig changes, until
/// SIGINT/SIGTERM.
async fn run_wrapper(mut opts: options::Options) -> ExitCode {
    // Every run, reloads included, starts from the command line options;
    // the run merges the config file itself.
    let base = opts.clone();
    let (tx, mut changes) = tokio::sync::mpsc::unbounded_channel::<String>();
    let mut config_watch = None;
    if !opts.config.is_empty() {
        let file = server::clean_path(&opts.config);
        match std::fs::read_to_string(&file) {
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    tracing::error!(error = %e, file = %opts.config, "Options configuration file not found at startup");
                } else {
                    tracing::error!(error = %e, file = %opts.config, "Error reading options configuration file");
                }
                if !opts.continue_without_config {
                    return ExitCode::FAILURE;
                }
            }
            Ok(text) => {
                let mut merged = opts.clone();
                match merged.merge_config_yaml(&text) {
                    Err(e) => {
                        tracing::error!(error = %e, file = %opts.config, "failed to unmarshal options configuration file");
                        if !opts.continue_without_config {
                            return ExitCode::FAILURE;
                        }
                    }
                    Ok(()) => {
                        if !merged.custom_resource_config_file_deprecated.is_empty() {
                            tracing::warn!(
                                "The 'custom_resource_config_file' config key is deprecated and will be removed in a future release; use 'custom_resource_state_config_file' instead"
                            );
                            merged.apply_deprecated_crs_file_alias();
                        }
                        opts = merged;
                    }
                }
            }
        }
        match reload::watch_file(&opts.config, tx.clone()) {
            Ok(w) => config_watch = Some(w),
            Err(e) => {
                tracing::error!(error = %e, file = %opts.config, "Failed to watch options configuration file")
            }
        }
    }
    let mut crc_watch = reload::MovableWatch::new("Custom resource configuration file");
    let mut kubeconfig_watch = reload::MovableWatch::new("kubeconfig file");
    if !opts.custom_resource_config_file.is_empty() {
        if let Err(e) = reload::read_yaml_file(&opts.custom_resource_config_file) {
            let file = &opts.custom_resource_config_file;
            let err = format!("open {file}: {}", server::go_io_error(&e));
            if e.kind() == std::io::ErrorKind::NotFound {
                tracing::error!(err = %err, file = %file, "Custom resource configuration file not found at startup");
            } else {
                tracing::error!(error = %e, file = %file, "Error reading Custom resource configuration file");
            }
            if !opts.continue_without_custom_resource_config_file {
                return ExitCode::FAILURE;
            }
        }
        crc_watch.watch(&opts.custom_resource_config_file, &tx);
    }
    if !opts.kubeconfig.is_empty() {
        if let Err(e) = reload::read_yaml_file(&opts.kubeconfig) {
            if e.kind() == std::io::ErrorKind::NotFound {
                tracing::error!(error = %e, file = %opts.kubeconfig, "kubeconfig file not found");
            } else {
                tracing::error!(error = %e, file = %opts.kubeconfig, "Error reading kubeconfig file");
            }
            return ExitCode::FAILURE;
        }
        kubeconfig_watch.watch(&opts.kubeconfig, &tx);
    }
    tracing::info!("Starting kube-state-metrics");
    let mut runs = tokio::task::JoinSet::new();
    let mut current = start_run(&mut runs, &base);
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    // `KSMRunOrDie`: a failed run is logged and ends the process; a run
    // stopped by its context ends without an error.
    let failed = |r: Result<(bool, Result<(), String>), tokio::task::JoinError>| {
        let e = match r {
            Ok((true, _) | (false, Ok(()))) => return false,
            Ok((false, Err(e))) => e,
            Err(e) => e.to_string(),
        };
        logging::error_go("wrapper.go:48", "Failed to run kube-state-metrics", &e);
        true
    };
    'main: loop {
        tokio::select! {
            () = &mut shutdown => break,
            Some(r) = runs.join_next(), if !runs.is_empty() => {
                if failed(r) {
                    return ExitCode::FAILURE;
                }
            }
            Some(name) = changes.recv() => {
                tracing::info!(name = %name, "Changes detected");
                // Follow the files the restarted run will use.
                if let Some(next) = reloaded_options(&base) {
                    crc_watch.watch(&next.custom_resource_config_file, &tx);
                    kubeconfig_watch.watch(&next.kubeconfig, &tx);
                }
                current.1.store(true, Ordering::SeqCst);
                current.0.cancel();
                // Wait for the ports to be released.
                let wait = tokio::time::sleep(Duration::from_secs(3));
                tokio::pin!(wait);
                loop {
                    tokio::select! {
                        () = &mut shutdown => break 'main,
                        () = &mut wait => break,
                        Some(r) = runs.join_next(), if !runs.is_empty() => {
                            if failed(r) {
                                return ExitCode::FAILURE;
                            }
                        }
                    }
                }
                tracing::info!("Restarting kube-state-metrics, metrics will be reset");
                current = start_run(&mut runs, &base);
            }
        }
    }
    drop(config_watch);
    tracing::info!("Received termination signal, shutting down kube-state-metrics");
    current.0.cancel();
    while let Some(r) = runs.join_next().await {
        if failed(r) {
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// Starts one run on a fresh copy of the base options; returns its
/// cancellation token and reload flag.
fn start_run(
    runs: &mut tokio::task::JoinSet<(bool, Result<(), String>)>,
    base: &options::Options,
) -> (CancellationToken, Arc<AtomicBool>) {
    let cancel = CancellationToken::new();
    let reload = Arc::new(AtomicBool::new(false));
    let (c, r, o) = (cancel.clone(), Arc::clone(&reload), base.clone());
    runs.spawn(async move {
        let res = server::run(o, c.clone(), r).await;
        (c.is_cancelled(), res)
    });
    (cancel, reload)
}

/// `reloadedOptions`: the options a run starting from `base` uses, with the
/// options config file merged in; `None` when that file cannot be read or
/// parsed.
fn reloaded_options(base: &options::Options) -> Option<options::Options> {
    let mut opts = base.clone();
    if !opts.config.is_empty() {
        let text = std::fs::read_to_string(server::clean_path(&opts.config)).ok()?;
        opts.merge_config_yaml(&text).ok()?;
    }
    opts.apply_deprecated_crs_file_alias();
    Some(opts)
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(s) => s,
                Err(_) => {
                    let _ = ctrl_c.await;
                    return;
                }
            };
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}
