//! Custom resource state metrics against Go's output (testdata/golden/crs.jsonl
//! and flect.jsonl, recorded by tools/golden-go/crs/run.sh). Compared per
//! ADR-003: headers exactly, the lines of each family as a multiset.

use ksm_core::metric::go_float_string;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Gvk {
    group: String,
    version: String,
    kind: String,
}

#[derive(Deserialize)]
struct GoResource {
    gvk: Gvk,
    resource_name: String,
    prefix: String,
    #[serde(default)]
    error: Option<String>,
    families: Vec<String>,
    headers: Vec<String>,
    outputs: Vec<Vec<String>>,
}

#[derive(Deserialize)]
struct Record {
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    config: String,
    #[serde(default)]
    objects: Option<Vec<Value>>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    resources: Vec<GoResource>,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    nil_is_zero: bool,
    #[serde(default)]
    value: Option<String>,
}

fn golden(name: &str) -> String {
    let path = format!(
        "{}/../../testdata/golden/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn sorted_lines(s: &str) -> Vec<&str> {
    let mut v: Vec<&str> = s.lines().collect();
    v.sort_unstable();
    v
}

fn go_format_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".to_owned()
    } else if f == f64::INFINITY {
        "+Inf".to_owned()
    } else if f == f64::NEG_INFINITY {
        "-Inf".to_owned()
    } else {
        go_float_string(f)
    }
}

/// Checks one metrics record; returns the number of compared families.
fn check_metrics(
    r: &Record,
    failures: &mut Vec<String>,
    yaml_msg_diffs: &mut Vec<String>,
) -> usize {
    let name = &r.name;
    let resources = match ksm_crs::from_config(&r.config) {
        Ok(res) => res,
        Err(e) => {
            match &r.error {
                Some(want) if *want == e => {}
                Some(want) => {
                    // yaml.v3's own messages (line numbers, Go type names)
                    // are not reproduced; the prefix and the failure are.
                    if !e.starts_with("failed to parse Custom Resource State metrics: ") {
                        failures.push(format!("{name}: error prefix: {e}"));
                    }
                    yaml_msg_diffs.push(format!("{name}:\n    go:   {want}\n    rust: {e}"));
                }
                None => failures.push(format!("{name}: unexpected error {e}")),
            }
            return 0;
        }
    };
    if let Some(want) = &r.error {
        failures.push(format!("{name}: want error {want}, got none"));
        return 0;
    }
    if resources.len() != r.resources.len() {
        failures.push(format!(
            "{name}: {} resources, want {}",
            resources.len(),
            r.resources.len()
        ));
        return 0;
    }
    let mut n = 0;
    for (i, (res, want)) in resources.iter().zip(&r.resources).enumerate() {
        let gvk = res.gvk();
        if (gvk.group.as_str(), gvk.version.as_str(), gvk.kind.as_str())
            != (
                want.gvk.group.as_str(),
                want.gvk.version.as_str(),
                want.gvk.kind.as_str(),
            )
        {
            failures.push(format!("{name}[{i}]: gvk {gvk:?}"));
        }
        if res.get_resource_name() != want.resource_name {
            failures.push(format!(
                "{name}[{i}]: resource name {:?}, want {:?}",
                res.get_resource_name(),
                want.resource_name
            ));
        }
        if res.get_metric_name_prefix() != want.prefix {
            failures.push(format!(
                "{name}[{i}]: prefix {:?}, want {:?}",
                res.get_metric_name_prefix(),
                want.prefix
            ));
        }
        let families = match (res.families(), &want.error) {
            (Ok(f), None) => f,
            (Err(e), Some(w)) => {
                if e != *w {
                    failures.push(format!("{name}[{i}]: error\n    got:  {e}\n    want: {w}"));
                }
                continue;
            }
            (Ok(_), Some(w)) => {
                failures.push(format!("{name}[{i}]: want error {w}"));
                continue;
            }
            (Err(e), None) => {
                failures.push(format!("{name}[{i}]: unexpected error {e}"));
                continue;
            }
        };
        let names: Vec<&str> = families.iter().map(|f| f.name.as_str()).collect();
        if names != want.families {
            failures.push(format!("{name}[{i}]: families {names:?}"));
            continue;
        }
        let headers = ksm_core::generator::extract_headers(&families);
        if headers != want.headers {
            failures.push(format!(
                "{name}[{i}]: headers\n    got:  {headers:?}\n    want: {:?}",
                want.headers
            ));
        }
        for (oi, (obj, want_out)) in r.objects.iter().flatten().zip(&want.outputs).enumerate() {
            for (g, want_fam) in families.iter().zip(want_out) {
                let mut buf = Vec::new();
                (g.generate)(obj).append_bytes(&g.name, &mut buf);
                let got = String::from_utf8(buf).unwrap_or_default();
                if sorted_lines(&got) != sorted_lines(want_fam) {
                    failures.push(format!(
                        "{name}[{i}] object {oi} family {}:\n--- got\n{got}--- want\n{want_fam}",
                        g.name
                    ));
                }
                n += 1;
            }
        }
    }
    n
}

#[test]
fn crs_matches_go() {
    let data = golden("crs.jsonl");
    let mut failures = Vec::new();
    let mut yaml_msg_diffs = Vec::new();
    let (mut cases, mut families, mut tofloat) = (0, 0, 0);
    for line in data.lines() {
        let r: Record = serde_json::from_str(line).expect("valid record");
        match r.kind.as_str() {
            "metrics" => {
                cases += 1;
                families += check_metrics(&r, &mut failures, &mut yaml_msg_diffs);
            }
            "tofloat" => {
                tofloat += 1;
                let got = ksm_crs::to_float64(Some(&r.input), r.nil_is_zero);
                match (&got, &r.value, &r.error) {
                    (Ok(v), Some(want), None) if go_format_float(*v) == *want => {}
                    (Err(e), None, Some(want)) if e == want => {}
                    _ => failures.push(format!(
                        "toFloat64({}, {}) = {got:?}, want value {:?} error {:?}",
                        r.input, r.nil_is_zero, r.value, r.error
                    )),
                }
            }
            other => panic!("unknown record kind {other}"),
        }
    }
    eprintln!(
        "crs golden: {cases} configs, {families} rendered families, {tofloat} toFloat64 vectors; \
         {} yaml.v3 error messages differ in wording:",
        yaml_msg_diffs.len()
    );
    for d in &yaml_msg_diffs {
        eprintln!("  {d}");
    }
    assert!(cases > 0 && families > 0 && tofloat > 0);
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn flect_matches_go() {
    #[derive(Deserialize)]
    struct Case {
        r#in: String,
        out: String,
    }
    let data = golden("flect.jsonl");
    let mut failures = Vec::new();
    let mut n = 0;
    for line in data.lines() {
        let c: Case = serde_json::from_str(line).expect("valid record");
        n += 1;
        let got = ksm_crs::flect::pluralize(&c.r#in);
        if got != c.out {
            failures.push(format!(
                "Pluralize({:?}) = {got:?}, want {:?}",
                c.r#in, c.out
            ));
        }
    }
    assert!(n > 0);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
