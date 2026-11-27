//! Micro-benchmarks (ADR-008): metric generation per object for the busiest
//! stores, and writing a store's exposition text. Objects come from the store
//! goldens (testdata/fixtures/golden/objects.json).

use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use ksm_core::generator::{extract_headers, render};
use ksm_core::store::{Headers, MetricsStore, MetricsWriter, sanitize_headers};
use serde::de::DeserializeOwned;

fn objects<K: DeserializeOwned>(kind: &str) -> Vec<K> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/fixtures/golden/objects.json"
    );
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("fixtures")).expect("json");
    v["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|o| o["kind"] == kind)
        .filter_map(|o| serde_json::from_value(o.clone()).ok())
        .collect()
}

fn bench_store<K: DeserializeOwned + 'static>(
    c: &mut Criterion,
    kind: &str,
    families: fn(&[String], &[String]) -> Vec<ksm_core::FamilyGenerator<K>>,
) {
    let objs: Vec<K> = objects(kind);
    if objs.is_empty() {
        return;
    }
    let all = vec!["*".to_owned()];
    let fams = families(&all, &all);
    c.bench_function(&format!("render/{kind} ({} objects)", objs.len()), |b| {
        b.iter(|| {
            for o in &objs {
                black_box(render(&fams, o));
            }
        });
    });
    // A store of 10k objects (the fixtures repeated), written once per iteration.
    let store = Arc::new(MetricsStore::default());
    for i in 0..10_000 {
        let o = &objs[i % objs.len()];
        store.add(&i.to_string(), || render(&fams, o));
    }
    let w = MetricsWriter::new(
        kind,
        Arc::new(Headers::new(extract_headers(&fams))),
        vec![store],
    );
    c.bench_function(&format!("write_all/{kind} x10000"), |b| {
        b.iter(|| {
            let h = sanitize_headers(true, &[&w]);
            let mut out = Vec::with_capacity(1 << 20);
            w.write_all(&h[0], &mut out);
            black_box(out.len())
        });
    });
}

fn benches(c: &mut Criterion) {
    use ksm_stores::*;
    bench_store(c, "Pod", pod::families);
    bench_store(c, "Node", node::families);
    bench_store(c, "Deployment", deployment::families);
    bench_store(c, "Service", service::families);
    bench_store(c, "ConfigMap", configmap::families);
    c.bench_function("labels/sanitize+snake 1k keys", |b| {
        let keys: Vec<String> = (0..1000)
            .map(|i| format!("app.kubernetes.io/SomeKey-{i}"))
            .collect();
        b.iter(|| {
            for k in &keys {
                black_box(ksm_core::labels::label_name("label", k));
            }
        });
    });
}

criterion_group!(group, benches);
criterion_main!(group);
