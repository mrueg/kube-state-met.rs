//! Quantity parsing and accessors against Go (testdata/golden/quantity.jsonl,
//! recorded by tools/golden-go/quantity).

use ksm_core::metric::go_float_string;
use ksm_stores::quantity;

#[test]
fn quantities_match_go() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/golden/quantity.jsonl"
    );
    let data = std::fs::read_to_string(path).expect("golden file");
    let mut bad = Vec::new();
    let mut n = 0;
    for line in data.lines() {
        let v: serde_json::Value = serde_json::from_str(line).expect("valid line");
        let input = v["in"].as_str().expect("in");
        n += 1;
        match (quantity::parse(input), v["ok"].as_bool() == Some(true)) {
            (Ok(q), true) => {
                let mut errs = Vec::new();
                if Some(q.value()) != v["value"].as_i64() {
                    errs.push(format!("value {}", q.value()));
                }
                if Some(q.milli_value()) != v["milli"].as_i64() {
                    errs.push(format!("milli {}", q.milli_value()));
                }
                if Some(go_float_string(q.as_approximate_f64()).as_str()) != v["approx"].as_str() {
                    errs.push(format!(
                        "approx {}",
                        go_float_string(q.as_approximate_f64())
                    ));
                }
                if Some(go_float_string(quantity::ksm_float(&q)).as_str())
                    != v["ksm_float"].as_str()
                {
                    errs.push(format!(
                        "ksm_float {}",
                        go_float_string(quantity::ksm_float(&q))
                    ));
                }
                if !errs.is_empty() {
                    bad.push(format!("{input:?}: {} (want {v})", errs.join(", ")));
                }
            }
            (Err(e), false) => {
                if Some(e.to_string().as_str()) != v["err"].as_str() {
                    bad.push(format!("{input:?}: error {e} (want {v})"));
                }
            }
            (got, _) => bad.push(format!("{input:?}: got {got:?} (want {v})")),
        }
    }
    assert!(n > 800, "only {n} cases");
    assert!(
        bad.is_empty(),
        "{} mismatches:\n{}",
        bad.len(),
        bad.join("\n")
    );
}
