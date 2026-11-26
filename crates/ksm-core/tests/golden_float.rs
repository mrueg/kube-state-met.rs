//! Float formatting against Go's `strconv.FormatFloat(f, 'g', -1, 64)`
//! (testdata/golden/float_g.jsonl, recorded by tools/golden-go/floatfmt).

#[derive(serde::Deserialize)]
struct Case {
    bits: String,
    out: String,
}

#[test]
fn floats_match_go() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/golden/float_g.jsonl"
    );
    let data = std::fs::read_to_string(path).expect("golden file");
    let mut n = 0;
    for line in data.lines() {
        let c: Case = serde_json::from_str(line).expect("valid case");
        let bits = u64::from_str_radix(c.bits.trim_start_matches("0x"), 16).expect("hex bits");
        let f = f64::from_bits(bits);
        let got = ksm_core::metric::go_float_string(f);
        // KSM writes -0 as 0 before formatting; the formatter itself keeps the sign.
        let want = if f == 0.0 && f.is_sign_negative() {
            "-0".to_owned()
        } else {
            c.out.clone()
        };
        assert_eq!(got, want, "bits {}", c.bits);
        n += 1;
    }
    assert!(n > 3000, "only {n} cases");
}
