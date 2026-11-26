//! Every store against Go's own output for the objects of kube-state-metrics'
//! store tests (testdata/golden/stores.jsonl, recorded by
//! tools/golden-go/stores/run.sh). Comparison per ADR-003: headers exactly,
//! and inside each family the same lines in any order.
//!
//! `GOLDEN_ONLY=<familiesFn>` limits the run to one store.

use ksm_core::generator::{FamilyGenerator, extract_headers, render};
use ksm_stores::*;
use serde::de::DeserializeOwned;

#[derive(serde::Deserialize)]
struct Record {
    test: String,
    families_fn: String,
    object: serde_json::Value,
    allow_annotations: Option<Vec<String>>,
    allow_labels: Option<Vec<String>>,
    headers: Vec<String>,
    #[serde(default)]
    output: Vec<String>,
}

fn sorted_lines(s: &str) -> Vec<&str> {
    let mut v: Vec<&str> = s.lines().collect();
    v.sort_unstable();
    v
}

fn check<K: DeserializeOwned + 'static>(
    rec: &Record,
    families: fn(&[String], &[String]) -> Vec<FamilyGenerator<K>>,
) -> Result<(), String> {
    let obj: K =
        serde_json::from_value(rec.object.clone()).map_err(|e| format!("decode object: {e}"))?;
    let ann = rec.allow_annotations.clone().unwrap_or_default();
    let lab = rec.allow_labels.clone().unwrap_or_default();
    let fams = families(&ann, &lab);
    let headers = extract_headers(&fams);
    if headers != rec.headers {
        let diff: Vec<String> = headers
            .iter()
            .zip(&rec.headers)
            .filter(|(a, b)| a != b)
            .map(|(a, b)| format!("  rust: {a:?}\n  go:   {b:?}"))
            .collect();
        return Err(format!(
            "headers differ ({} rust vs {} go families)\n{}",
            headers.len(),
            rec.headers.len(),
            diff.join("\n")
        ));
    }
    let rendered = render(&fams, &obj);
    let mut errs = Vec::new();
    for (i, want) in rec.output.iter().enumerate() {
        let got = std::str::from_utf8(rendered.family(i)).map_err(|e| e.to_string())?;
        let (g, w) = (sorted_lines(got), sorted_lines(want));
        if g != w {
            let missing: Vec<&&str> = w.iter().filter(|l| !g.contains(l)).collect();
            let extra: Vec<&&str> = g.iter().filter(|l| !w.contains(l)).collect();
            errs.push(format!(
                "family {}:\n  missing (go only): {missing:#?}\n  extra (rust only): {extra:#?}",
                fams[i].name
            ));
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("\n"))
    }
}

macro_rules! dispatch {
    ($($module:ident, $ty:ty, $gofn:literal;)*) => {
        fn run(rec: &Record) -> Option<Result<(), String>> {
            match rec.families_fn.as_str() {
                $($gofn => {
                    if !$module::PORTED { return None; }
                    Some(check::<$ty>(rec, $module::families))
                })*
                other => Some(Err(format!("no store for {other}"))),
            }
        }
    };
}
for_each_resource!(dispatch);

#[test]
fn stores_match_go() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/golden/stores.jsonl"
    );
    let data = std::fs::read_to_string(path).expect("golden file");
    let only = std::env::var("GOLDEN_ONLY").ok();
    let (mut ok, mut skipped, mut failed) = (0, 0, Vec::new());
    for (i, line) in data.lines().enumerate() {
        let rec: Record = serde_json::from_str(line).expect("valid record");
        if only.as_deref().is_some_and(|o| o != rec.families_fn) {
            continue;
        }
        match run(&rec) {
            None => skipped += 1,
            Some(Ok(())) => ok += 1,
            Some(Err(e)) => failed.push(format!(
                "line {} {} ({}):\n{e}",
                i + 1,
                rec.families_fn,
                rec.test
            )),
        }
    }
    eprintln!(
        "golden stores: {ok} match, {} differ, {skipped} not ported",
        failed.len()
    );
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
    if std::env::var_os("GOLDEN_REQUIRE_ALL").is_some() {
        assert_eq!(skipped, 0, "stores not ported yet");
    }
}
