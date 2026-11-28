//! Label helpers against Go's output (testdata/golden/labels.jsonl, recorded
//! by tools/golden-go/stores/run.sh).

use std::collections::BTreeMap;

use ksm_core::labels::{
    create_prometheus_label_keys_values, label_name, map_to_prometheus_labels, sanitize_label_name,
    to_snake_case,
};

#[derive(serde::Deserialize)]
struct Case {
    #[serde(rename = "fn")]
    func: String,
    #[serde(default)]
    r#in: BTreeMap<String, String>,
    #[serde(default)]
    arg: String,
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    keys: Option<Vec<String>>,
    #[serde(default)]
    values: Option<Vec<String>>,
}

#[test]
fn labels_match_go() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/golden/labels.jsonl"
    );
    let data = std::fs::read_to_string(path).expect("golden file");
    let mut n = 0;
    for (i, line) in data.lines().enumerate() {
        let c: Case = serde_json::from_str(line).expect("valid case");
        let want_keys = c.keys.unwrap_or_default();
        let want_values = c.values.unwrap_or_default();
        let (keys, values): (Vec<String>, Vec<String>) = match c.func.as_str() {
            "SanitizeLabelName" => (vec![sanitize_label_name(&c.arg).into_owned()], vec![]),
            "toSnakeCase" => (vec![to_snake_case(&c.arg).into_owned()], vec![]),
            "labelName" => (vec![label_name("label", &c.arg)], vec![]),
            "mapToPrometheusLabels" => {
                let (k, v) = map_to_prometheus_labels(&c.r#in, "label");
                (
                    k.into_iter().map(|k| k.into_owned()).collect(),
                    v.into_iter().map(|v| v.into_owned()).collect(),
                )
            }
            "createPrometheusLabelKeysValues" => {
                let (k, v) =
                    create_prometheus_label_keys_values("annotation", Some(&c.r#in), &c.allow);
                (
                    k.into_iter().map(|k| k.into_owned()).collect(),
                    v.into_iter().map(|v| v.into_owned()).collect(),
                )
            }
            other => panic!("unknown fn {other}"),
        };
        assert_eq!(keys, want_keys, "line {}: {line}", i + 1);
        assert_eq!(values, want_values, "line {}: {line}", i + 1);
        n += 1;
    }
    assert!(n > 900, "only {n} cases");
}
