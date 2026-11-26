//! kube-state-metrics in Rust (kube-state-met.rs): listens to the Kubernetes
//! API server and generates metrics about the state of the objects.
//! Go entry points: `main.go`, `internal/wrapper.go`.

mod builder;
mod client;
mod crs;
mod discovery;
mod handler;
mod logging;
mod options;
mod process;
mod server;
mod telemetry;
mod watch;
mod watch_dynamic;
mod web;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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

/// `RunKubeStateMetricsWrapper`: merges the options config file, runs until
/// SIGINT/SIGTERM.
async fn run_wrapper(mut opts: options::Options) -> ExitCode {
    if !opts.config.is_empty() {
        match std::fs::read_to_string(&opts.config) {
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
                            if merged.custom_resource_config_file.is_empty() {
                                merged.custom_resource_config_file =
                                    merged.custom_resource_config_file_deprecated.clone();
                            }
                        }
                        opts.custom_resource_config_file = merged.custom_resource_config_file;
                        opts.kubeconfig = merged.kubeconfig;
                    }
                }
            }
        }
    }
    if !opts.kubeconfig.is_empty()
        && let Err(e) = std::fs::read_to_string(&opts.kubeconfig)
    {
        if e.kind() == std::io::ErrorKind::NotFound {
            tracing::error!(error = %e, file = %opts.kubeconfig, "kubeconfig file not found");
        } else {
            tracing::error!(error = %e, file = %opts.kubeconfig, "Error reading kubeconfig file");
        }
        return ExitCode::FAILURE;
    }
    tracing::info!("Starting kube-state-metrics");
    let cancel = CancellationToken::new();
    let reload = Arc::new(AtomicBool::new(false));
    let run = tokio::spawn(server::run(opts, cancel.clone(), Arc::clone(&reload)));
    tokio::pin!(run);
    tokio::select! {
        r = &mut run => return finish(r),
        () = shutdown_signal() => {}
    }
    tracing::info!("Received termination signal, shutting down kube-state-metrics");
    cancel.cancel();
    finish(run.await)
}

fn finish(r: Result<Result<(), String>, tokio::task::JoinError>) -> ExitCode {
    match r {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(e)) => {
            logging::error_go("wrapper.go:48", "Failed to run kube-state-metrics", &e);
            ExitCode::FAILURE
        }
        Err(e) => {
            logging::error_go(
                "wrapper.go:48",
                "Failed to run kube-state-metrics",
                &e.to_string(),
            );
            ExitCode::FAILURE
        }
    }
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
