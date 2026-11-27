//! Unit tests for the options layer.
//!
//! `tests/cli_cases.json` records the Go binary (argv, `$SHELL`, stdout, exit
//! code, error text); `tests/options_go_cases.json` records Go's
//! `pkg/options`, `yaml.Unmarshal`, `strconv` and `time` through a small Go
//! harness. `tools/golden-go/options/run.sh` regenerates both, and the help
//! and completion texts next to this file.

use super::*;
use serde_json::{Value, json};

const CLI_CASES: &str = include_str!("../../tests/cli_cases.json");
const GO_CASES: &str = include_str!("../../tests/options_go_cases.json");

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|s| s.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// What the Go binary prints to stdout for a non-run command.
fn render(cmd: &Command, shell: Option<&str>) -> String {
    match cmd {
        Command::Run(_) => "<run>".to_owned(),
        Command::Help => help_text().to_owned(),
        Command::Version => "<version>\n".to_owned(),
        Command::Completion(shell) => completion_text(shell).to_owned(),
        Command::VersionHelp => version_help_text().to_owned(),
        Command::HelpCommandHelp => help_command_help_text().to_owned(),
        Command::CompletionHelp => completion_help_text(shell),
    }
}

#[test]
fn cli_cases_match_go() {
    let cases: Value = serde_json::from_str(CLI_CASES).unwrap();
    let cases = cases.as_array().unwrap();
    assert!(cases.len() > 150);
    for case in cases {
        let args = strings(&case["args"]);
        let shell = case["shell"].as_str();
        let got = parse(&args);
        if let Some(err) = case["err"].as_str() {
            assert_eq!(got, Err(err.to_owned()), "args {args:?}");
            assert_eq!(case["exit"], 1);
        } else if case["run"] == true {
            assert!(matches!(got, Ok(Command::Run(_))), "args {args:?}: {got:?}");
        } else {
            let cmd = got.unwrap_or_else(|e| panic!("args {args:?}: {e}"));
            assert_eq!(
                render(&cmd, shell),
                case["stdout"].as_str().unwrap(),
                "args {args:?}"
            );
            assert_eq!(case["exit"], 0);
        }
    }
}

/// The options after flag parsing only (no validation).
fn flags_only(args: &[String]) -> Options {
    let mut opts = Options::default();
    let rest = ROOT
        .parse(args, &mut |flag, value| opts.set_flag(flag.name, value))
        .unwrap();
    assert!(rest.is_empty());
    opts
}

fn nanos(d: Duration) -> i64 {
    i64::try_from(d.as_nanos()).unwrap()
}

/// The options in the shape the case generator writes Go's options in.
fn canonical(o: &Options) -> Value {
    let r = o.auto_go_memlimit_ratio;
    #[expect(clippy::cast_possible_truncation, reason = "integral and small")]
    let ratio = if r.is_finite() && r.fract() == 0.0 && r.abs() < 1e15 {
        // encoding/json writes integral floats without a fraction.
        json!(r as i64)
    } else if r.is_finite() {
        json!(r)
    } else if o.auto_go_memlimit_ratio.is_nan() {
        json!("NaN")
    } else if o.auto_go_memlimit_ratio > 0.0 {
        json!("+Inf")
    } else {
        json!("-Inf")
    };
    json!({
        "annotations_allow_list": o.annotations_allow_list,
        "labels_allow_list": o.labels_allow_list,
        "metric_allowlist": o.metric_allowlist,
        "metric_denylist": o.metric_denylist,
        "metric_opt_in_list": o.metric_opt_in_list,
        "resources": o.resources,
        "apiserver": o.apiserver,
        "custom_resource_config": o.custom_resource_config,
        "custom_resource_config_file": o.custom_resource_config_file,
        "custom_resource_config_file_deprecated": o.custom_resource_config_file_deprecated,
        "continue_without_custom_resource_config_file": o.continue_without_custom_resource_config_file,
        "host": o.host,
        "kubeconfig": o.kubeconfig,
        "namespace": o.namespace,
        "node": o.node,
        "pod": o.pod,
        "tls_config": o.tls_config,
        "telemetry_host": o.telemetry_host,
        "config": o.config,
        "continue_without_config": o.continue_without_config,
        "namespaces": o.namespaces,
        "namespaces_denylist": o.namespaces_denylist,
        "auto_go_memlimit_ratio": ratio,
        "port": o.port,
        "telemetry_port": o.telemetry_port,
        "total_shards": o.total_shards,
        "server_read_timeout": nanos(o.server_read_timeout),
        "server_write_timeout": nanos(o.server_write_timeout),
        "server_idle_timeout": nanos(o.server_idle_timeout),
        "server_read_header_timeout": nanos(o.server_read_header_timeout),
        "shutdown_delay": nanos(o.shutdown_delay),
        "auto_go_memlimit_refresh_interval": o.auto_go_memlimit_refresh_interval_nanos,
        "shard": o.shard,
        "auto_go_memlimit": o.auto_go_memlimit,
        "custom_resources_only": o.custom_resources_only,
        "enable_gzip_encoding": o.enable_gzip_encoding,
        "help": o.help,
        "track_unscheduled_pods": o.track_unscheduled_pods,
        "use_api_server_cache": o.use_api_server_cache,
        "object_limit": o.object_limit,
        "auth_filter": o.auth_filter,
    })
}

/// Drops the `line N: ` and ` at line N` parts of yaml.v3 type errors, which
/// the Rust port does not know.
fn without_lines(err: &str) -> String {
    let mut out = String::new();
    let mut rest = err;
    while let Some(i) = rest.find("line ") {
        let (head, tail) = rest.split_at(i);
        let digits = tail[5..].bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            out.push_str(&rest[..i + 5]);
            rest = &rest[i + 5..];
            continue;
        }
        let after = &tail[5 + digits..];
        if let Some(a) = after.strip_prefix(": ") {
            out.push_str(head);
            rest = a;
        } else {
            out.push_str(head.strip_suffix(" at ").unwrap_or(head));
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn check_error(got: Result<(), String>, want: &Value, what: &str) {
    match want.as_str() {
        None => assert_eq!(got, Ok(()), "{what}"),
        Some(want) => {
            let got = got.expect_err(what);
            if want.starts_with("yaml: unmarshal errors:") || !want.starts_with("yaml: ") {
                assert_eq!(got, without_lines(want), "{what}");
            } else {
                // A YAML syntax error: the parser's wording differs.
                assert!(got.starts_with("yaml: "), "{what}: {got}");
            }
        }
    }
}

#[test]
fn go_cases_match() {
    let cases: Value = serde_json::from_str(GO_CASES).unwrap();
    let cases = cases.as_array().unwrap();
    assert!(cases.len() > 300);
    let failures: Vec<String> = cases
        .iter()
        .filter(|case| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check_go_case(case))).is_err()
        })
        .map(|case| case["req"].to_string())
        .collect();
    assert!(failures.is_empty(), "failing cases: {failures:#?}");
}

fn check_go_case(case: &Value) {
    {
        let req = &case["req"];
        let res = &case["res"];
        let what = req.to_string();
        let s = req["s"].as_str().unwrap_or_default();
        match req["op"].as_str().unwrap() {
            op @ ("yaml" | "configure") => {
                let mut opts = flags_only(&strings(&req["args"]));
                let yaml = req["yaml"].as_str().unwrap();
                if op == "yaml" {
                    check_error(opts.merge_config_yaml(yaml), &res["err"], &what);
                } else {
                    opts.configure_resources_and_metrics(yaml);
                }
                assert_eq!(canonical(&opts), res["opts"], "{what}");
            }
            "dur" => match res["err"].as_str() {
                Some(err) => assert_eq!(goparse::parse_duration(s), Err(err.to_owned()), "{what}"),
                None => {
                    let ns = res["ns"].as_i64().unwrap();
                    assert_eq!(goparse::parse_duration(s), Ok(ns), "{what}");
                    assert_eq!(goparse::format_duration(ns), res["str"], "{what}");
                }
            },
            op @ ("int" | "uint") => {
                let got = if op == "int" {
                    let bits = u32::try_from(req["bits"].as_u64().unwrap()).unwrap();
                    goparse::parse_int(s, 0, bits)
                        .map(|v| v.to_string())
                        .map_err(|e| num_error("ParseInt", s, e))
                } else {
                    goparse::parse_uint(s, 0, 64)
                        .map(|v| v.to_string())
                        .map_err(|e| num_error("ParseUint", s, e))
                };
                match res["err"].as_str() {
                    Some(err) => assert_eq!(got, Err(err.to_owned()), "{what}"),
                    None => assert_eq!(got.as_deref(), Ok(res["v"].as_str().unwrap()), "{what}"),
                }
            }
            "float" => {
                let got = goparse::parse_float(s);
                match res["err"].as_str() {
                    Some(err) => assert_eq!(
                        got.map_err(|e| num_error("ParseFloat", s, e)),
                        Err(err.to_owned()),
                        "{what}"
                    ),
                    None => {
                        let want = match res["v"].as_str().unwrap() {
                            "+Inf" => f64::INFINITY,
                            "-Inf" => f64::NEG_INFINITY,
                            v => v.parse::<f64>().unwrap(),
                        };
                        let got = got.unwrap();
                        assert!(
                            got.to_bits() == want.to_bits() || (got.is_nan() && want.is_nan()),
                            "{what}: {got} != {want}"
                        );
                    }
                }
            }
            "bool" => {
                let got = goparse::parse_bool(s).map_err(|e| num_error("ParseBool", s, e));
                match res["err"].as_str() {
                    Some(err) => assert_eq!(got, Err(err.to_owned()), "{what}"),
                    None => assert_eq!(got, Ok(res["v"].as_bool().unwrap()), "{what}"),
                }
            }
            "quote" => assert_eq!(goparse::quote(s), res["v"], "{what}"),
            "fmtf" => assert_eq!(
                goparse::format_f(req["f"].as_f64().unwrap()),
                res["v"],
                "{what}"
            ),
            "merge" => {
                let sels = strings(&req["sels"]);
                let sels: Vec<&str> = sels.iter().map(String::as_str).collect();
                match res["err"].as_str() {
                    Some(err) => {
                        assert_eq!(merge_field_selectors(&sels), Err(err.to_owned()), "{what}");
                    }
                    None => assert_eq!(
                        merge_field_selectors(&sels).as_deref(),
                        Ok(res["v"].as_str().unwrap()),
                        "{what}"
                    ),
                }
            }
            "selectors" => {
                let opts = Options {
                    namespaces_denylist: strings(&req["ns"]),
                    node: req["node"].as_str().unwrap().to_owned(),
                    ..Options::default()
                };
                assert_eq!(opts.exclude_ns_field_selector(), res["exclude"], "{what}");
                assert_eq!(opts.node_field_selector(), res["node"], "{what}");
            }
            "labels" => match res["err"].as_str() {
                Some(err) => assert_eq!(parse_labels_allow_list(s), Err(err.to_owned()), "{what}"),
                None => assert_eq!(
                    json!(parse_labels_allow_list(s).unwrap()),
                    res["v"],
                    "{what}"
                ),
            },
            other => panic!("unknown op {other}"),
        }
    }
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

fn run(a: &[&str]) -> Options {
    match parse(&args(a)) {
        Ok(Command::Run(opts)) => opts,
        other => panic!("{a:?}: {other:?}"),
    }
}

#[test]
fn defaults() {
    let opts = run(&[]);
    assert_eq!(opts, Options::default());
    assert_eq!(opts.port, 8080);
    assert_eq!(opts.telemetry_port, 8081);
    assert_eq!(opts.host, "::");
    assert_eq!(opts.total_shards, 1);
    assert_eq!(opts.server_idle_timeout, Duration::from_secs(300));
    assert_eq!(opts.server_read_header_timeout, Duration::from_secs(5));
    assert!((opts.auto_go_memlimit_ratio - 0.9).abs() < f64::EPSILON);
    assert!(opts.klog.logtostderr);
    assert_eq!(opts.klog.log_file_max_size, 1800);
    assert_eq!(opts.get_namespaces(), vec![String::new()]);
    assert_eq!(opts.exclude_ns_field_selector(), "");
    assert_eq!(opts.node_field_selector(), "");
}

#[test]
fn flag_forms() {
    assert_eq!(run(&["--port", "1"]).port, 1);
    assert_eq!(run(&["--port=2"]).port, 2);
    assert_eq!(run(&["-v", "3"]).verbosity, 3);
    assert_eq!(run(&["-v=4"]).verbosity, 4);
    assert_eq!(run(&["-v5"]).verbosity, 5);
    assert_eq!(run(&["--v=6"]).verbosity, 6);
    assert!(run(&["--auth-filter"]).auth_filter);
    assert!(!run(&["--auth-filter", "--auth-filter=false"]).auth_filter);
    assert_eq!(
        parse(&args(&["--port=1", "--", "--port=2"])),
        Err("unknown command \"--port=2\" for \"kube-state-metrics\"".to_owned())
    );
    let o = run(&["--resources=pods", "--resources=secrets,pods"]);
    assert_eq!(
        o.resources,
        BTreeSet::from(["pods".to_owned(), "secrets".to_owned()])
    );
    let o = run(&["--namespaces=a", "--namespaces=b,a"]);
    assert_eq!(o.namespaces, args(&["a", "b", "a"]));
    assert_eq!(o.get_namespaces(), args(&["a", "b", "a"]));
    let o = run(&[
        "--metric-labels-allowlist=pods=[a]",
        "--metric-labels-allowlist=nodes=[b]",
    ]);
    assert_eq!(
        o.labels_allow_list,
        BTreeMap::from([("nodes".to_owned(), args(&["b"]))])
    );
    let o = run(&[
        "--stderrthreshold=warning",
        "--vmodule=a=1",
        "--log_backtrace_at=x.go:1",
    ]);
    assert_eq!(o.klog.stderrthreshold, 1);
    assert_eq!(o.klog.vmodule, "a=1");
    assert_eq!(o.klog.log_backtrace_at, "x.go:1");
}

#[test]
fn negative_durations() {
    let o = run(&["--server-read-timeout=-5s", "--shutdown-delay=-1h"]);
    assert_eq!(o.server_read_timeout, Duration::ZERO);
    assert_eq!(o.shutdown_delay, Duration::ZERO);
    assert_eq!(
        parse(&args(&["--auto-gomemlimit-refresh-interval=-1m30s"])),
        Err(
            "value for --auto-gomemlimit-refresh-interval=-1m30s must be equal or greater than 0"
                .to_owned()
        )
    );
    let mut o = Options::default();
    o.merge_config_yaml("auto-gomemlimit-refresh-interval: -2s")
        .unwrap();
    assert_eq!(
        o.validate(),
        Err(
            "value for --auto-gomemlimit-refresh-interval=-2s must be equal or greater than 0"
                .to_owned()
        )
    );
}

#[test]
fn validate_order() {
    // Go checks the node first, then the ratio, refresh interval, object
    // limit, total shards and shard.
    let o = Options {
        node: "n".to_owned(),
        resources: BTreeSet::from(["secrets".to_owned()]),
        auto_go_memlimit_ratio: 0.0,
        object_limit: -1,
        total_shards: 0,
        ..Options::default()
    };
    assert_eq!(
        o.validate(),
        Err("resource secrets can't be sharded by field selector spec.nodeName".to_owned())
    );
    let o = Options {
        object_limit: -1,
        total_shards: 0,
        ..Options::default()
    };
    assert_eq!(
        o.validate(),
        Err("value for --object-limit=-1 must be equal or greater than 0".to_owned())
    );
}

#[test]
fn field_selectors() {
    let o = Options {
        namespaces_denylist: args(&["kube-system", "a,b"]),
        node: "node-1".to_owned(),
        ..Options::default()
    };
    assert_eq!(
        o.exclude_ns_field_selector(),
        "metadata.namespace!=kube-system,metadata.namespace!=a\\,b"
    );
    assert_eq!(o.node_field_selector(), "spec.nodeName=node-1");
    assert_eq!(
        merge_field_selectors(&[&o.exclude_ns_field_selector(), &o.node_field_selector()]),
        Ok(
            "metadata.namespace!=a\\,b,metadata.namespace!=kube-system,spec.nodeName=node-1"
                .to_owned()
        )
    );
    assert_eq!(merge_field_selectors(&[]), Ok(String::new()));
    assert_eq!(merge_field_selectors(&["", ""]), Ok(String::new()));
}

#[test]
fn config_merge() {
    let mut o = run(&["--resources=configmaps", "--port=1", "--namespaces=x"]);
    o.merge_config_yaml(
        "resources: {pods: {}}\nnamespaces: [a]\nhost: h\nlabels_allow_list: {pods: [app]}\n",
    )
    .unwrap();
    assert_eq!(
        o.resources,
        BTreeSet::from(["configmaps".to_owned(), "pods".to_owned()])
    );
    assert_eq!(o.namespaces, args(&["a"]));
    assert_eq!(o.port, 1);
    assert_eq!(o.host, "h");
    o.configure_resources_and_metrics("resources: {pods: {}}\n");
    assert_eq!(o.resources, BTreeSet::from(["pods".to_owned()]));

    let mut o = Options::default();
    o.merge_config_yaml("custom_resource_config_file: /old")
        .unwrap();
    o.configure_resources_and_metrics("custom_resource_config_file: /old");
    assert_eq!(o.custom_resource_config_file, "/old");

    let mut o = Options::default();
    assert_eq!(
        o.merge_config_yaml("labels_allow_list: {pods: ['a*b*']}"),
        Err(ERR_LABELS_ALLOW_LIST_MULTIPLE_WILDCARDS.to_owned())
    );
}

#[test]
fn texts() {
    assert!(help_text().starts_with("kube-state-metrics is a simple service"));
    assert!(help_text().ends_with("for more information about a command.\n"));
    assert!(help_text().contains(&format!("Defaults to \"{}\"", DEFAULT_RESOURCES.join(","))));
    assert_eq!(completion_text("tcsh"), "");
    assert!(completion_text("bash").starts_with("# bash completion for kube-state-metrics"));
    assert_eq!(completion_help_text(Some("")), "\n");
    assert!(
        completion_help_text(Some("/usr/bin/zsh"))
            .starts_with("Generate the autocompletion script for the zsh shell.")
    );
    let v = version_text("v2.19.0", "abc", "main");
    let lines: Vec<&str> = v.lines().collect();
    assert_eq!(
        lines[0],
        "kube-state-metrics, version v2.19.0 (branch: main, revision: abc)"
    );
    assert_eq!(lines[1], "  build user:       ");
    assert_eq!(lines[2], "  build date:       ");
    assert!(lines[3].starts_with("  go version:       rustc "));
    assert!(lines[4].starts_with("  platform:         linux/") || !cfg!(target_os = "linux"));
    assert_eq!(lines[5], "  tags:             unknown");
    assert!(!v.ends_with('\n'));
}
