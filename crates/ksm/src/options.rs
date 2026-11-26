//! Command line and options layer: a port of `pkg/options` (flags, cobra
//! commands, validation, field selectors) and of the options config file
//! merge in `internal/wrapper.go` and `pkg/app/server.go`.
//!
//! Every flag keeps its Go spelling, default and help text (ADR-004), and
//! parsing follows `spf13/pflag` and `spf13/cobra`, error texts included.

#![expect(dead_code, reason = "used once main.rs dispatches")]

mod fields;
mod flagset;
mod goparse;
mod yamlnode;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use flagset::{FlagSet, FlagSpec};
use goparse::{NumErr, num_error};
use yamlnode::{Decoded, Decoder, Node};

/// `options.DefaultResources`, sorted.
pub const DEFAULT_RESOURCES: &[&str] = &[
    "certificatesigningrequests",
    "configmaps",
    "cronjobs",
    "daemonsets",
    "deployments",
    "endpointslices",
    "horizontalpodautoscalers",
    "ingresses",
    "jobs",
    "leases",
    "limitranges",
    "mutatingadmissionpolicies",
    "mutatingadmissionpolicybindings",
    "mutatingwebhookconfigurations",
    "namespaces",
    "networkpolicies",
    "nodes",
    "persistentvolumeclaims",
    "persistentvolumes",
    "poddisruptionbudgets",
    "pods",
    "replicasets",
    "replicationcontrollers",
    "resourcequotas",
    "secrets",
    "services",
    "statefulsets",
    "storageclasses",
    "validatingadmissionpolicies",
    "validatingadmissionpolicybindings",
    "validatingwebhookconfigurations",
    "volumeattachments",
];

/// `metav1.NamespaceAll`.
pub const NAMESPACE_ALL: &str = "";

/// `options.LabelWildcard`.
pub const LABEL_WILDCARD: &str = "*";

/// `options.MaxPartialWildcardsPerLabel`.
const MAX_PARTIAL_WILDCARDS_PER_LABEL: usize = 1;

const ERR_LABELS_ALLOW_LIST_FORMAT: &str =
    "invalid format, metric=[label1,label2,labeln...],metricN=[]";
const ERR_LABELS_ALLOW_LIST_MULTIPLE_WILDCARDS: &str =
    "invalid format, only one wildcard per label allowed";

/// The klog flags. kube-state-metrics only registers them; apart from `-v`
/// (see [`Options::verbosity`]) they have no effect in the Rust port
/// (ADR-002), but they are parsed and validated like klog does.
#[derive(Clone, Debug, PartialEq)]
pub struct KlogFlags {
    pub add_dir_header: bool,
    pub alsologtostderr: bool,
    /// Severity number (INFO=0, WARNING=1, ERROR=2, FATAL=3).
    pub alsologtostderrthreshold: i32,
    pub legacy_stderr_threshold_behavior: bool,
    /// `file:N`, or empty.
    pub log_backtrace_at: String,
    pub log_dir: String,
    pub log_file: String,
    pub log_file_max_size: u64,
    pub logtostderr: bool,
    pub one_output: bool,
    pub skip_headers: bool,
    pub skip_log_headers: bool,
    /// Severity number (INFO=0, WARNING=1, ERROR=2, FATAL=3).
    pub stderrthreshold: i32,
    /// `pattern=N,...` as given.
    pub vmodule: String,
}

impl Default for KlogFlags {
    fn default() -> Self {
        Self {
            add_dir_header: false,
            alsologtostderr: false,
            alsologtostderrthreshold: 0,
            // kube-state-metrics sets these two in AddFlags.
            legacy_stderr_threshold_behavior: false,
            log_backtrace_at: String::new(),
            log_dir: String::new(),
            log_file: String::new(),
            log_file_max_size: 1800,
            logtostderr: true,
            one_output: false,
            skip_headers: false,
            skip_log_headers: false,
            stderrthreshold: 0,
            vmodule: String::new(),
        }
    }
}

/// The configurable parameters of kube-state-metrics (`options.Options`).
///
/// Go durations can be negative; [`Duration`] cannot. A negative value parsed
/// for a server timeout or the shutdown delay is stored as zero, which Go
/// treats the same way. A negative `--auto-gomemlimit-refresh-interval` is
/// remembered in `auto_go_memlimit_refresh_interval_negative`, so
/// [`Options::validate`] rejects it as Go does.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub annotations_allow_list: BTreeMap<String, Vec<String>>,
    pub labels_allow_list: BTreeMap<String, Vec<String>>,
    pub metric_allowlist: BTreeSet<String>,
    pub metric_denylist: BTreeSet<String>,
    pub metric_opt_in_list: BTreeSet<String>,
    pub resources: BTreeSet<String>,

    pub apiserver: String,
    pub custom_resource_config: String,
    /// `--custom-resource-state-config-file`, yaml key `custom_resource_state_config_file`.
    pub custom_resource_config_file: String,
    /// Deprecated yaml key `custom_resource_config_file`.
    pub custom_resource_config_file_deprecated: String,
    pub continue_without_custom_resource_config_file: bool,
    pub host: String,
    pub kubeconfig: String,
    /// `--pod-namespace`.
    pub namespace: String,
    pub node: String,
    pub pod: String,
    pub tls_config: String,
    pub telemetry_host: String,

    pub config: String,
    pub continue_without_config: bool,

    pub namespaces: Vec<String>,
    pub namespaces_denylist: Vec<String>,
    pub auto_go_memlimit_ratio: f64,
    pub port: i64,
    pub telemetry_port: i64,
    pub total_shards: i64,
    pub server_read_timeout: Duration,
    pub server_write_timeout: Duration,
    pub server_idle_timeout: Duration,
    pub server_read_header_timeout: Duration,
    pub shutdown_delay: Duration,

    pub auto_go_memlimit_refresh_interval: Duration,
    /// Whether the refresh interval was given as a negative duration; its
    /// magnitude is in `auto_go_memlimit_refresh_interval`.
    pub auto_go_memlimit_refresh_interval_negative: bool,

    pub shard: i32,
    pub auto_go_memlimit: bool,
    pub custom_resources_only: bool,
    pub enable_gzip_encoding: bool,
    pub help: bool,
    pub track_unscheduled_pods: bool,
    pub use_api_server_cache: bool,
    pub object_limit: i64,
    pub auth_filter: bool,

    /// klog `-v`.
    pub verbosity: i32,
    /// The other klog flags.
    pub klog: KlogFlags,
}

impl Default for Options {
    /// The values after `NewOptions()` and `AddFlags()`: the flag defaults.
    fn default() -> Self {
        Self {
            annotations_allow_list: BTreeMap::new(),
            labels_allow_list: BTreeMap::new(),
            metric_allowlist: BTreeSet::new(),
            metric_denylist: BTreeSet::new(),
            metric_opt_in_list: BTreeSet::new(),
            resources: BTreeSet::new(),
            apiserver: String::new(),
            custom_resource_config: String::new(),
            custom_resource_config_file: String::new(),
            custom_resource_config_file_deprecated: String::new(),
            continue_without_custom_resource_config_file: false,
            host: "::".to_owned(),
            kubeconfig: String::new(),
            namespace: String::new(),
            node: String::new(),
            pod: String::new(),
            tls_config: String::new(),
            telemetry_host: "::".to_owned(),
            config: String::new(),
            continue_without_config: false,
            namespaces: Vec::new(),
            namespaces_denylist: Vec::new(),
            auto_go_memlimit_ratio: 0.9,
            port: 8080,
            telemetry_port: 8081,
            total_shards: 1,
            server_read_timeout: Duration::from_secs(60),
            server_write_timeout: Duration::from_secs(60),
            server_idle_timeout: Duration::from_secs(300),
            server_read_header_timeout: Duration::from_secs(5),
            shutdown_delay: Duration::ZERO,
            auto_go_memlimit_refresh_interval: Duration::ZERO,
            auto_go_memlimit_refresh_interval_negative: false,
            shard: 0,
            auto_go_memlimit: false,
            custom_resources_only: false,
            enable_gzip_encoding: false,
            help: false,
            track_unscheduled_pods: false,
            use_api_server_cache: false,
            object_limit: 0,
            auth_filter: false,
            verbosity: 0,
            klog: KlogFlags::default(),
        }
    }
}

/// What the command line asks for.
#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "built once per process; the API keeps Options unboxed"
)]
pub enum Command {
    /// Run kube-state-metrics with these (validated) options.
    Run(Options),
    /// Print [`help_text`] to stdout and exit 0.
    Help,
    /// Print [`version_text`] and a newline to stdout and exit 0.
    Version,
    /// Print [`completion_text`] of this shell to stdout and exit 0.
    Completion(String),
    /// `version -h`: print [`version_help_text`] to stdout and exit 0.
    VersionHelp,
    /// `help -h`: print [`help_command_help_text`] to stdout and exit 0.
    HelpCommandHelp,
    /// `completion -h`: print [`completion_help_text`] for `$SHELL` to stdout
    /// and exit 0.
    CompletionHelp,
}

/// `kube-state-metrics -h` (and `help`), byte for byte.
pub fn help_text() -> &'static str {
    include_str!("options/help.txt")
}

/// `kube-state-metrics version -h`.
pub fn version_help_text() -> &'static str {
    include_str!("options/help_version.txt")
}

/// `kube-state-metrics help -h`.
pub fn help_command_help_text() -> &'static str {
    include_str!("options/help_help.txt")
}

/// The script cobra generates for `completion bash|zsh|fish`; empty for any
/// other shell (Go prints nothing and exits 0).
pub fn completion_text(shell: &str) -> &'static str {
    match shell {
        "bash" => include_str!("options/completion.bash"),
        "zsh" => include_str!("options/completion.zsh"),
        "fish" => include_str!("options/completion.fish"),
        _ => "",
    }
}

const AUTOLOAD_ZSH: &str = r#"Generate the autocompletion script for the zsh shell.

If shell completion is not already enabled in your environment you will need
to enable it.  You can execute the following once:

        echo "autoload -U compinit; compinit" >> ~/.zshrc

To load completions in your current shell session:

        source <(kube-state-metrics completion zsh); compdef _kube-state-metrics kube-state-metrics

To load completions for every new session, execute once:

#### Linux:

        kube-state-metrics completion zsh > "${fpath[1]}/_kube-state-metrics"

#### macOS:

        kube-state-metrics completion zsh > $(brew --prefix)/share/zsh/site-functions/_kube-state-metrics

You will need to start a new shell for this setup to take effect.

Usage:
  kube-state-metrics completion zsh [flags]

Flags:
  --no-descriptions   disable completion descriptions
"#;

const AUTOLOAD_BASH: &str = r"Generate the autocompletion script for the bash shell.

This script depends on the 'bash-completion' package.
If it is not installed already, you can install it via your OS's package manager.

To load completions in your current shell session:

        source <(kube-state-metrics completion bash)

To load completions for every new session, execute once:

#### Linux:

        kube-state-metrics completion bash > /etc/bash_completion.d/kube-state-metrics

#### macOS:

        kube-state-metrics completion bash > $(brew --prefix)/etc/bash_completion.d/kube-state-metrics

You will need to start a new shell for this setup to take effect.

Usage:
  kube-state-metrics completion bash

Flags:
  --no-descriptions   disable completion descriptions
";

const AUTOLOAD_FISH: &str = r"Generate the autocompletion script for the fish shell.

To load completions in your current shell session:

        kube-state-metrics completion fish | source

To load completions for every new session, execute once:

        kube-state-metrics completion fish > ~/.config/fish/completions/kube-state-metrics.fish

You will need to start a new shell for this setup to take effect.

Usage:
  kube-state-metrics completion fish [flags]

Flags:
  --no-descriptions   disable completion descriptions
";

/// `options.FetchLoadInstructions`.
pub fn fetch_load_instructions(shell: &str) -> &'static str {
    match shell {
        "zsh" => AUTOLOAD_ZSH,
        "bash" => AUTOLOAD_BASH,
        "fish" => AUTOLOAD_FISH,
        _ => "",
    }
}

/// What `completion -h` prints, given `$SHELL` (`None` when unset).
pub fn completion_help_text(shell_env: Option<&str>) -> String {
    match shell_env {
        Some(path) => {
            let shell = path.rsplit('/').next().unwrap_or(path);
            format!("{}\n", fetch_load_instructions(shell))
        }
        None => format!(
            "SHELL environment variable not set, falling back to bash\n{}\n",
            fetch_load_instructions("bash")
        ),
    }
}

/// `version.Print("kube-state-metrics")` from prometheus/common, without the
/// trailing newline (the caller prints it with one, as Go's `version`
/// command does). The `go version` line reports the rustc version.
pub fn version_text(version: &str, revision: &str, branch: &str) -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        "powerpc64" => "ppc64",
        "loongarch64" => "loong64",
        other => other,
    };
    let text = format!(
        "\nkube-state-metrics, version {version} (branch: {branch}, revision: {revision})\n  build user:       \n  build date:       \n  go version:       {}\n  platform:         {}/{arch}\n  tags:             unknown\n",
        env!("KSM_RUSTC_VERSION"),
        std::env::consts::OS,
    );
    text.trim().to_owned()
}

// ---------------------------------------------------------------------------
// Flags

/// Every flag of the root command, as registered by `AddFlags` (klog flags
/// included).
static ROOT_FLAGS: &[FlagSpec] = &[
    FlagSpec::boolean("add_dir_header"),
    FlagSpec::boolean("alsologtostderr"),
    FlagSpec::value("alsologtostderrthreshold"),
    FlagSpec::value("apiserver"),
    FlagSpec::boolean("auth-filter"),
    FlagSpec::boolean("auto-gomemlimit"),
    FlagSpec::value("auto-gomemlimit-ratio"),
    FlagSpec::value("auto-gomemlimit-refresh-interval"),
    FlagSpec::value("config"),
    FlagSpec::boolean("continue-without-config"),
    FlagSpec::boolean("continue-without-custom-resource-state-config-file"),
    FlagSpec::value("custom-resource-state-config"),
    FlagSpec::value("custom-resource-state-config-file"),
    FlagSpec::boolean("custom-resource-state-only"),
    FlagSpec::boolean("enable-gzip-encoding"),
    FlagSpec::boolean("help").short(b'h'),
    FlagSpec::value("host"),
    FlagSpec::value("kubeconfig"),
    FlagSpec::boolean("legacy_stderr_threshold_behavior"),
    FlagSpec::value("log_backtrace_at"),
    FlagSpec::value("log_dir"),
    FlagSpec::value("log_file"),
    FlagSpec::value("log_file_max_size"),
    FlagSpec::boolean("logtostderr"),
    FlagSpec::value("metric-allowlist"),
    FlagSpec::value("metric-annotations-allowlist"),
    FlagSpec::value("metric-denylist"),
    FlagSpec::value("metric-labels-allowlist"),
    FlagSpec::value("metric-opt-in-list"),
    FlagSpec::value("namespaces"),
    FlagSpec::value("namespaces-denylist"),
    FlagSpec::value("node"),
    FlagSpec::value("object-limit"),
    FlagSpec::boolean("one_output"),
    FlagSpec::value("pod"),
    FlagSpec::value("pod-namespace"),
    FlagSpec::value("port"),
    FlagSpec::value("resources"),
    FlagSpec::value("server-idle-timeout"),
    FlagSpec::value("server-read-header-timeout"),
    FlagSpec::value("server-read-timeout"),
    FlagSpec::value("server-write-timeout"),
    FlagSpec::value("shard"),
    FlagSpec::value("shutdown-delay"),
    FlagSpec::boolean("skip_headers"),
    FlagSpec::boolean("skip_log_headers"),
    FlagSpec::value("stderrthreshold"),
    FlagSpec::value("telemetry-host"),
    FlagSpec::value("telemetry-port"),
    FlagSpec::value("tls-config"),
    FlagSpec::value("total-shards"),
    FlagSpec::boolean("track-unscheduled-pods"),
    FlagSpec::boolean("use-apiserver-cache"),
    FlagSpec::value("v").short(b'v'),
    FlagSpec::value("vmodule"),
];

/// Subcommands only have cobra's `-h, --help`.
static SUBCOMMAND_FLAGS: &[FlagSpec] = &[FlagSpec::boolean("help").short(b'h')];

const ROOT: FlagSet = FlagSet { specs: ROOT_FLAGS };
const SUBCOMMAND: FlagSet = FlagSet {
    specs: SUBCOMMAND_FLAGS,
};

fn pflag_bool(value: &str) -> Result<bool, String> {
    goparse::parse_bool(value).map_err(|e| num_error("ParseBool", value, e))
}

fn pflag_int(value: &str, bits: u32) -> Result<i64, String> {
    goparse::parse_int(value, 0, bits).map_err(|e| num_error("ParseInt", value, e))
}

/// Go's `flag` package: bool flags report `parse error`.
fn goflag_bool(value: &str) -> Result<bool, String> {
    goparse::parse_bool(value).map_err(|_| "parse error".to_owned())
}

/// Go's `flag` package `uint` flag.
fn goflag_uint(value: &str) -> Result<u64, String> {
    goparse::parse_uint(value, 0, 64).map_err(|e| match e {
        NumErr::Syntax => "parse error".to_owned(),
        NumErr::Range => "value out of range".to_owned(),
    })
}

/// klog's `severityValue.Set`.
fn klog_severity(value: &str) -> Result<i32, String> {
    let upper: String = value.chars().flat_map(char::to_uppercase).collect();
    if let Some(i) = ["INFO", "WARNING", "ERROR", "FATAL"]
        .iter()
        .position(|n| *n == upper)
    {
        return Ok(i32::try_from(i).unwrap_or_default());
    }
    goparse::parse_int(value, 10, 32)
        .map(|v| i32::try_from(v).unwrap_or_default())
        .map_err(|e| num_error("ParseInt", value, e))
}

/// klog's `moduleSpec.Set` (validation only).
fn klog_vmodule(value: &str) -> Result<(), String> {
    const SYNTAX: &str = "syntax error: expect comma-separated list of filename=N";
    for pat in value.split(',') {
        if pat.is_empty() {
            continue;
        }
        let parts: Vec<&str> = pat.split('=').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(SYNTAX.to_owned());
        }
        let v = goparse::parse_int(parts[1], 10, 32).map_err(|_| SYNTAX.to_owned())?;
        if v < 0 {
            return Err("negative value for vmodule level".to_owned());
        }
    }
    Ok(())
}

/// klog's `traceLocation.Set` (validation only).
fn klog_trace_location(value: &str) -> Result<(), String> {
    const SYNTAX: &str = "syntax error: expect file.go:234";
    if value.is_empty() {
        return Ok(());
    }
    let parts: Vec<&str> = value.split(':').collect();
    if parts.len() != 2 || !parts[0].contains('.') {
        return Err(SYNTAX.to_owned());
    }
    let v = goparse::parse_int(parts[1], 10, 64).map_err(|_| SYNTAX.to_owned())?;
    if v <= 0 {
        return Err("negative or zero value for level".to_owned());
    }
    Ok(())
}

/// Splits a duration in Go nanoseconds into a [`Duration`] and its sign.
fn split_nanos(nanos: i64) -> (Duration, bool) {
    (Duration::from_nanos(nanos.unsigned_abs()), nanos < 0)
}

/// A Go duration flag/field whose negative values behave like zero.
fn non_negative(nanos: i64) -> Duration {
    if nanos < 0 {
        Duration::ZERO
    } else {
        Duration::from_nanos(nanos.unsigned_abs())
    }
}

/// `MetricSet.Set` / `ResourceSet.Set`: adds comma-separated entries.
fn add_to_set(set: &mut BTreeSet<String>, value: &str) {
    for item in value.split(',') {
        let item = item.trim();
        if !item.is_empty() {
            set.insert(item.to_owned());
        }
    }
}

/// `NamespaceList.Set`: appends comma-separated entries.
fn append_to_list(list: &mut Vec<String>, value: &str) {
    for item in value.split(',') {
        let item = item.trim();
        if !item.is_empty() {
            list.push(item.to_owned());
        }
    }
}

/// Go's `strings.TrimSpace`, which trims Unicode white space.
fn trim_space(s: &str) -> &str {
    s.trim()
}

fn too_many_wildcards(map: &BTreeMap<String, Vec<String>>) -> bool {
    map.values().any(|group| {
        group
            .iter()
            .any(|label| label.matches(LABEL_WILDCARD).count() > MAX_PARTIAL_WILDCARDS_PER_LABEL)
    })
}

/// `LabelsAllowList.Set`: parses `res=[a,b],res2=[c]`, replacing the list.
pub fn parse_labels_allow_list(value: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut rest = value;
    while !rest.is_empty() {
        let (name, after) = rest.split_once("=[").ok_or(ERR_LABELS_ALLOW_LIST_FORMAT)?;
        let (labels, after) = after.split_once(']').ok_or(ERR_LABELS_ALLOW_LIST_FORMAT)?;
        let name = trim_space(name);
        if name.is_empty() || name.contains(['[', ']', ',', '=']) || labels.contains(['[', '=']) {
            return Err(ERR_LABELS_ALLOW_LIST_FORMAT.to_owned());
        }
        let entry = m.entry(name.to_owned()).or_default();
        entry.clear();
        if !trim_space(labels).is_empty() {
            for label in labels.split(',') {
                let label = trim_space(label);
                if label.is_empty() {
                    return Err(ERR_LABELS_ALLOW_LIST_FORMAT.to_owned());
                }
                entry.push(label.to_owned());
            }
        }
        if after.is_empty() {
            break;
        }
        match after.strip_prefix(',') {
            Some(r) if !r.is_empty() => rest = r,
            _ => return Err(ERR_LABELS_ALLOW_LIST_FORMAT.to_owned()),
        }
    }
    if too_many_wildcards(&m) {
        return Err(ERR_LABELS_ALLOW_LIST_MULTIPLE_WILDCARDS.to_owned());
    }
    Ok(m)
}

impl Options {
    /// Sets one root command flag (`flag.Value.Set`). Returns the cause of an
    /// invalid value.
    fn set_flag(&mut self, name: &str, value: &str) -> Result<(), String> {
        let k = &mut self.klog;
        match name {
            "add_dir_header" => k.add_dir_header = goflag_bool(value)?,
            "alsologtostderr" => k.alsologtostderr = goflag_bool(value)?,
            "alsologtostderrthreshold" => k.alsologtostderrthreshold = klog_severity(value)?,
            "legacy_stderr_threshold_behavior" => {
                k.legacy_stderr_threshold_behavior = goflag_bool(value)?;
            }
            "log_backtrace_at" => {
                klog_trace_location(value)?;
                k.log_backtrace_at = value.to_owned();
            }
            "log_dir" => k.log_dir = value.to_owned(),
            "log_file" => k.log_file = value.to_owned(),
            "log_file_max_size" => k.log_file_max_size = goflag_uint(value)?,
            "logtostderr" => k.logtostderr = goflag_bool(value)?,
            "one_output" => k.one_output = goflag_bool(value)?,
            "skip_headers" => k.skip_headers = goflag_bool(value)?,
            "skip_log_headers" => k.skip_log_headers = goflag_bool(value)?,
            "stderrthreshold" => k.stderrthreshold = klog_severity(value)?,
            "vmodule" => {
                klog_vmodule(value)?;
                k.vmodule = value.to_owned();
            }
            "v" => {
                self.verbosity = goparse::parse_int(value, 10, 32)
                    .map(|v| i32::try_from(v).unwrap_or_default())
                    .map_err(|e| num_error("ParseInt", value, e))?;
            }
            "apiserver" => self.apiserver = value.to_owned(),
            "auth-filter" => self.auth_filter = pflag_bool(value)?,
            "auto-gomemlimit" => self.auto_go_memlimit = pflag_bool(value)?,
            "auto-gomemlimit-ratio" => {
                self.auto_go_memlimit_ratio =
                    goparse::parse_float(value).map_err(|e| num_error("ParseFloat", value, e))?;
            }
            "auto-gomemlimit-refresh-interval" => {
                let nanos = goparse::parse_duration(value)?;
                (
                    self.auto_go_memlimit_refresh_interval,
                    self.auto_go_memlimit_refresh_interval_negative,
                ) = split_nanos(nanos);
            }
            "config" => self.config = value.to_owned(),
            "continue-without-config" => self.continue_without_config = pflag_bool(value)?,
            "continue-without-custom-resource-state-config-file" => {
                self.continue_without_custom_resource_config_file = pflag_bool(value)?;
            }
            "custom-resource-state-config" => self.custom_resource_config = value.to_owned(),
            "custom-resource-state-config-file" => {
                self.custom_resource_config_file = value.to_owned();
            }
            "custom-resource-state-only" => self.custom_resources_only = pflag_bool(value)?,
            "enable-gzip-encoding" => self.enable_gzip_encoding = pflag_bool(value)?,
            "help" => self.help = pflag_bool(value)?,
            "host" => self.host = value.to_owned(),
            "kubeconfig" => self.kubeconfig = value.to_owned(),
            "metric-allowlist" => add_to_set(&mut self.metric_allowlist, value),
            "metric-annotations-allowlist" => {
                self.annotations_allow_list = parse_labels_allow_list(value)?;
            }
            "metric-denylist" => add_to_set(&mut self.metric_denylist, value),
            "metric-labels-allowlist" => self.labels_allow_list = parse_labels_allow_list(value)?,
            "metric-opt-in-list" => add_to_set(&mut self.metric_opt_in_list, value),
            "namespaces" => append_to_list(&mut self.namespaces, value),
            "namespaces-denylist" => append_to_list(&mut self.namespaces_denylist, value),
            "node" => self.node = value.to_owned(),
            "object-limit" => self.object_limit = pflag_int(value, 64)?,
            "pod" => self.pod = value.to_owned(),
            "pod-namespace" => self.namespace = value.to_owned(),
            "port" => self.port = pflag_int(value, 64)?,
            "resources" => add_to_set(&mut self.resources, value),
            "server-idle-timeout" => {
                self.server_idle_timeout = non_negative(goparse::parse_duration(value)?);
            }
            "server-read-header-timeout" => {
                self.server_read_header_timeout = non_negative(goparse::parse_duration(value)?);
            }
            "server-read-timeout" => {
                self.server_read_timeout = non_negative(goparse::parse_duration(value)?);
            }
            "server-write-timeout" => {
                self.server_write_timeout = non_negative(goparse::parse_duration(value)?);
            }
            "shard" => {
                self.shard = i32::try_from(pflag_int(value, 32)?).unwrap_or_default();
            }
            "shutdown-delay" => self.shutdown_delay = non_negative(goparse::parse_duration(value)?),
            "telemetry-host" => self.telemetry_host = value.to_owned(),
            "telemetry-port" => self.telemetry_port = pflag_int(value, 64)?,
            "tls-config" => self.tls_config = value.to_owned(),
            "total-shards" => self.total_shards = pflag_int(value, 64)?,
            "track-unscheduled-pods" => self.track_unscheduled_pods = pflag_bool(value)?,
            "use-apiserver-cache" => self.use_api_server_cache = pflag_bool(value)?,
            _ => {}
        }
        Ok(())
    }

    /// `Options.Validate`, with Go's error texts.
    pub fn validate(&self) -> Result<(), String> {
        if !self.node.is_empty() {
            // Go walks a map here, so which offending resource it names is
            // random; this names the first in sorted order.
            if let Some(x) = self.resources.iter().find(|x| *x != "pods") {
                return Err(format!(
                    "resource {x} can't be sharded by field selector spec.nodeName"
                ));
            }
        }
        let ratio = self.auto_go_memlimit_ratio;
        if ratio.is_nan() || ratio <= 0.0 || ratio > 1.0 {
            return Err(format!(
                "value for --auto-gomemlimit-ratio={} must be greater than 0 and less than or equal to 1",
                goparse::format_f(ratio)
            ));
        }
        if self.auto_go_memlimit_refresh_interval_negative
            && !self.auto_go_memlimit_refresh_interval.is_zero()
        {
            let nanos = i64::try_from(self.auto_go_memlimit_refresh_interval.as_nanos())
                .unwrap_or(i64::MAX);
            return Err(format!(
                "value for --auto-gomemlimit-refresh-interval={} must be equal or greater than 0",
                goparse::format_duration(-nanos)
            ));
        }
        if self.object_limit < 0 {
            return Err(format!(
                "value for --object-limit={} must be equal or greater than 0",
                self.object_limit
            ));
        }
        if self.total_shards < 1 {
            return Err(format!(
                "value for --total-shards={} must be greater than 0",
                self.total_shards
            ));
        }
        if self.shard < 0 || i64::from(self.shard) >= self.total_shards {
            return Err(format!(
                "value for --shard={} must be between 0 and {} (--total-shards minus 1)",
                self.shard,
                self.total_shards - 1
            ));
        }
        Ok(())
    }

    /// Decodes an options config file onto these options, as
    /// `yaml.Unmarshal(configFile, opts)` does with gopkg.in/yaml.v3: keys
    /// missing from the file keep their value, `MetricSet`/`ResourceSet` maps
    /// get keys added, lists and label allow lists are replaced. As in Go,
    /// fields decoded before an error stay applied.
    pub fn merge_config_yaml(&mut self, yaml: &str) -> Result<(), String> {
        let Some(root) = yamlnode::parse_first_document(yaml)? else {
            return Ok(());
        };
        let mut d = Decoder::default();
        match &root {
            Node::Scalar(yamlnode::Scalar::Null, _) => {}
            Node::Map(entries) if d.check_unique_keys(entries) => {
                for (key, value) in entries {
                    let Node::Scalar(kind, key) = key else {
                        continue;
                    };
                    if *kind == yamlnode::Scalar::Null {
                        continue;
                    }
                    self.decode_field(&mut d, key, value)?;
                }
            }
            Node::Map(_) => {}
            other => d.type_error(other, "options.Options"),
        }
        d.into_result()
    }

    /// Decodes one config file key. Returns an error only for errors that
    /// abort decoding in Go (`LabelsAllowList.UnmarshalYAML`).
    fn decode_field(&mut self, d: &mut Decoder, key: &str, value: &Node) -> Result<(), String> {
        fn set<T>(slot: &mut T, decoded: Decoded<T>) {
            if let Decoded::Set(v) = decoded {
                *slot = v;
            }
        }
        match key {
            "annotations_allow_list" | "labels_allow_list" => {
                let n = d.errors.len();
                let decoded = d.string_list_map(value);
                if d.errors.len() != n {
                    return Ok(());
                }
                if let Decoded::Set(m) = decoded {
                    if !value.is_null() && too_many_wildcards(&m) {
                        return Err(ERR_LABELS_ALLOW_LIST_MULTIPLE_WILDCARDS.to_owned());
                    }
                    if key == "labels_allow_list" {
                        self.labels_allow_list = m;
                    } else {
                        self.annotations_allow_list = m;
                    }
                }
            }
            "metric_allowlist" => {
                d.set_into(value, "options.MetricSet", &mut self.metric_allowlist)
            }
            "metric_denylist" => d.set_into(value, "options.MetricSet", &mut self.metric_denylist),
            "metric_opt_in_list" => {
                d.set_into(value, "options.MetricSet", &mut self.metric_opt_in_list);
            }
            "resources" => d.set_into(value, "options.ResourceSet", &mut self.resources),
            "apiserver" => set(&mut self.apiserver, d.string(value, "string")),
            "custom_resource_config" => {
                set(&mut self.custom_resource_config, d.string(value, "string"));
            }
            "custom_resource_state_config_file" => {
                set(
                    &mut self.custom_resource_config_file,
                    d.string(value, "string"),
                );
            }
            "custom_resource_config_file" => set(
                &mut self.custom_resource_config_file_deprecated,
                d.string(value, "string"),
            ),
            "continue_without_custom_resource_state_config_file" => set(
                &mut self.continue_without_custom_resource_config_file,
                d.boolean(value),
            ),
            "host" => set(&mut self.host, d.string(value, "string")),
            "kubeconfig" => set(&mut self.kubeconfig, d.string(value, "string")),
            "namespace" => set(&mut self.namespace, d.string(value, "string")),
            "node" => set(&mut self.node, d.string(value, "options.NodeType")),
            "pod" => set(&mut self.pod, d.string(value, "string")),
            "tls_config" => set(&mut self.tls_config, d.string(value, "string")),
            "telemetry_host" => set(&mut self.telemetry_host, d.string(value, "string")),
            "config" => set(&mut self.config, d.string(value, "string")),
            "continue_without_config" => set(&mut self.continue_without_config, d.boolean(value)),
            "namespaces" => set(
                &mut self.namespaces,
                d.string_list(value, "options.NamespaceList"),
            ),
            "namespaces_denylist" => set(
                &mut self.namespaces_denylist,
                d.string_list(value, "options.NamespaceList"),
            ),
            "auto-gomemlimit-ratio" => set(&mut self.auto_go_memlimit_ratio, d.float(value)),
            "port" => set(&mut self.port, d.int(value, 64, "int")),
            "telemetry_port" => set(&mut self.telemetry_port, d.int(value, 64, "int")),
            "total_shards" => set(&mut self.total_shards, d.int(value, 64, "int")),
            "server_read_timeout" => {
                if let Decoded::Set(n) = d.duration(value) {
                    self.server_read_timeout = non_negative(n);
                }
            }
            "server_write_timeout" => {
                if let Decoded::Set(n) = d.duration(value) {
                    self.server_write_timeout = non_negative(n);
                }
            }
            "server_idle_timeout" => {
                if let Decoded::Set(n) = d.duration(value) {
                    self.server_idle_timeout = non_negative(n);
                }
            }
            "server_read_header_timeout" => {
                if let Decoded::Set(n) = d.duration(value) {
                    self.server_read_header_timeout = non_negative(n);
                }
            }
            "shutdown_delay" => {
                if let Decoded::Set(n) = d.duration(value) {
                    self.shutdown_delay = non_negative(n);
                }
            }
            "auto-gomemlimit-refresh-interval" => {
                if let Decoded::Set(n) = d.duration(value) {
                    (
                        self.auto_go_memlimit_refresh_interval,
                        self.auto_go_memlimit_refresh_interval_negative,
                    ) = split_nanos(n);
                }
            }
            "shard" => {
                if let Decoded::Set(v) = d.int(value, 32, "int32") {
                    self.shard = i32::try_from(v).unwrap_or_default();
                }
            }
            "auto-gomemlimit" => set(&mut self.auto_go_memlimit, d.boolean(value)),
            "custom_resources_only" => set(&mut self.custom_resources_only, d.boolean(value)),
            "enable_gzip_encoding" => set(&mut self.enable_gzip_encoding, d.boolean(value)),
            "help" => set(&mut self.help, d.boolean(value)),
            "track_unscheduled_pods" => set(&mut self.track_unscheduled_pods, d.boolean(value)),
            "use_api_server_cache" => set(&mut self.use_api_server_cache, d.boolean(value)),
            "object_limit" => set(&mut self.object_limit, d.int(value, 64, "int64")),
            "auth_filter" => set(&mut self.auth_filter, d.boolean(value)),
            _ => {}
        }
        Ok(())
    }

    /// `configureResourcesAndMetrics` (pkg/app/server.go): the resource,
    /// metric and label/annotation lists set in the config file replace the
    /// command line ones instead of being merged into them, and the
    /// deprecated `custom_resource_config_file` key fills in
    /// `custom_resource_state_config_file` when that is unset. A config file
    /// that does not decode changes nothing but the alias (Go logs the error).
    pub fn configure_resources_and_metrics(&mut self, yaml: &str) {
        let mut config = Options::default();
        if config.merge_config_yaml(yaml).is_ok() {
            if !config.resources.is_empty() {
                self.resources = config.resources;
            }
            if !config.metric_allowlist.is_empty() {
                self.metric_allowlist = config.metric_allowlist;
            }
            if !config.metric_denylist.is_empty() {
                self.metric_denylist = config.metric_denylist;
            }
            if !config.metric_opt_in_list.is_empty() {
                self.metric_opt_in_list = config.metric_opt_in_list;
            }
            if !config.labels_allow_list.is_empty() {
                self.labels_allow_list = config.labels_allow_list;
            }
            if !config.annotations_allow_list.is_empty() {
                self.annotations_allow_list = config.annotations_allow_list;
            }
        }
        if self.custom_resource_config_file.is_empty()
            && !self.custom_resource_config_file_deprecated.is_empty()
        {
            self.custom_resource_config_file = self.custom_resource_config_file_deprecated.clone();
        }
    }

    /// `NamespaceList.GetNamespaces`: the namespaces to watch, `[""]` (all)
    /// when none are set.
    pub fn get_namespaces(&self) -> Vec<String> {
        if self.namespaces.is_empty() {
            vec![NAMESPACE_ALL.to_owned()]
        } else {
            self.namespaces.clone()
        }
    }

    /// `GetExcludeNSFieldSelector(opts.NamespacesDenylist)`:
    /// `metadata.namespace!=a,metadata.namespace!=b`, or empty.
    pub fn exclude_ns_field_selector(&self) -> String {
        fields::Selector::And(
            self.namespaces_denylist
                .iter()
                .map(|ns| fields::Selector::NotHas("metadata.namespace".to_owned(), ns.clone()))
                .collect(),
        )
        .to_string()
    }

    /// `NodeType.GetNodeFieldSelector`: `spec.nodeName=<node>`, or empty.
    /// The caller skips it when `track_unscheduled_pods` is set.
    pub fn node_field_selector(&self) -> String {
        if self.node.is_empty() {
            String::new()
        } else {
            fields::Selector::Has("spec.nodeName".to_owned(), self.node.clone()).to_string()
        }
    }
}

/// `options.MergeFieldSelectors`: the AND of the field selectors, parsed and
/// printed by apimachinery's `fields` package (terms sorted, values escaped).
pub fn merge_field_selectors(selectors: &[&str]) -> Result<String, String> {
    let mut merged = String::new();
    for s in selectors {
        merged = merge_two_field_selectors(&merged, s)?;
    }
    Ok(merged)
}

/// `options.MergeTwoFieldSelectors`.
fn merge_two_field_selectors(s1: &str, s2: &str) -> Result<String, String> {
    let selector1 = fields::parse_selector(s1)?;
    let selector2 = fields::parse_selector(s2)?;
    if selector1.is_empty() {
        return Ok(selector2.to_string());
    }
    if selector2.is_empty() {
        return Ok(selector1.to_string());
    }
    Ok(fields::Selector::And(vec![selector1, selector2]).to_string())
}

// ---------------------------------------------------------------------------
// Commands (cobra)

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cmd {
    Root,
    Completion,
    Version,
    Help,
}

impl Cmd {
    fn flags(self) -> FlagSet {
        match self {
            Cmd::Root => ROOT,
            _ => SUBCOMMAND,
        }
    }
}

/// cobra's `stripFlags`: the arguments that are not flags or flag values.
fn strip_flags(args: &[String], flags: &FlagSet) -> Vec<String> {
    let mut commands = Vec::new();
    let mut rest = args;
    while let Some((s, tail)) = rest.split_first() {
        rest = tail;
        let takes_value =
            (s.starts_with("--") && !s.contains('=') && !flags.has_no_opt_def_val(&s[2..]))
                || (s.starts_with('-')
                    && !s.contains('=')
                    && s.len() == 2
                    && !flags.short_has_no_opt_def_val(&s[1..]));
        if s == "--" {
            break;
        } else if takes_value {
            if rest.len() <= 1 {
                break;
            }
            rest = &rest[1..];
        } else if !s.is_empty() && !s.starts_with('-') {
            commands.push(s.clone());
        }
    }
    commands
}

/// cobra's `argsMinusFirstX`.
fn args_minus_first_x(args: &[String], x: &str, flags: &FlagSet) -> Vec<String> {
    let mut pos = 0;
    while pos < args.len() {
        let s = &args[pos];
        let takes_value =
            (s.starts_with("--") && !s.contains('=') && !flags.has_no_opt_def_val(&s[2..]))
                || (s.starts_with('-')
                    && !s.contains('=')
                    && s.len() == 2
                    && !flags.short_has_no_opt_def_val(&s[1..]));
        if s == "--" {
            break;
        } else if takes_value {
            pos += 1;
        } else if !s.starts_with('-') && s == x {
            let mut out = args[..pos].to_vec();
            out.extend_from_slice(&args[pos + 1..]);
            return out;
        }
        pos += 1;
    }
    args.to_vec()
}

/// cobra's `Command.Find` from the root. Only the root has subcommands, and
/// every command but the root is a leaf, so `legacyArgs` never fails here.
fn find(args: &[String]) -> (Cmd, Vec<String>) {
    let Some(next) = strip_flags(args, &ROOT).into_iter().next() else {
        return (Cmd::Root, args.to_vec());
    };
    let cmd = match next.as_str() {
        "completion" | "comp" | "c" => Cmd::Completion,
        "version" => Cmd::Version,
        "help" => Cmd::Help,
        _ => return (Cmd::Root, args.to_vec()),
    };
    (cmd, args_minus_first_x(args, &next, &ROOT))
}

/// Parses the command line (without `argv[0]`) the way `InitCommand.Execute()`
/// does, running validation where Go's `PreRunE` does. `Err` holds the Go
/// error text (the `err="..."` part of the log line); Go exits with 1.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let (cmd, args) = find(args);
    if cmd == Cmd::Root {
        let mut opts = Options::default();
        let positional = ROOT.parse(&args, &mut |flag, value| opts.set_flag(flag.name, value))?;
        if opts.help {
            return Ok(Command::Help);
        }
        if let Some(first) = positional.first() {
            return Err(format!(
                "unknown command {} for \"kube-state-metrics\"",
                goparse::quote(first)
            ));
        }
        opts.validate()?;
        return Ok(Command::Run(opts));
    }

    let mut help = false;
    let positional = cmd.flags().parse(&args, &mut |_, value| {
        help = pflag_bool(value)?;
        Ok(())
    })?;
    if help {
        return Ok(help_for(cmd));
    }
    match cmd {
        Cmd::Version => Ok(Command::Version),
        Cmd::Completion => match positional.as_slice() {
            [shell] => Ok(Command::Completion(shell.clone())),
            _ => Err(format!("accepts 1 arg(s), received {}", positional.len())),
        },
        // `help [command]`: the help of the command the arguments name.
        Cmd::Help | Cmd::Root => Ok(help_for(find(&positional).0)),
    }
}

fn help_for(cmd: Cmd) -> Command {
    match cmd {
        Cmd::Root => Command::Help,
        Cmd::Version => Command::VersionHelp,
        Cmd::Completion => Command::CompletionHelp,
        Cmd::Help => Command::HelpCommandHelp,
    }
}

#[cfg(test)]
mod tests;
