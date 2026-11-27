//! Port of `pkg/metrics_store`: a store keeps the rendered metrics of every
//! object by UID (never the objects), and a writer writes the stores of one
//! resource so that same-named samples stay grouped under one header.

use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

/// The rendered families of one object: one buffer, family `i` ends at `ends[i]`.
#[derive(Debug, Default)]
pub struct Rendered {
    buf: Box<[u8]>,
    ends: Box<[u32]>,
}

impl Rendered {
    pub fn new(buf: Vec<u8>, ends: Vec<u32>) -> Self {
        Rendered {
            buf: buf.into_boxed_slice(),
            ends: ends.into_boxed_slice(),
        }
    }

    /// Bytes of family `i` (empty when the object has none).
    pub fn family(&self, i: usize) -> &[u8] {
        let end = self.ends.get(i).copied().unwrap_or(0) as usize;
        let start = if i == 0 {
            0
        } else {
            self.ends.get(i - 1).copied().unwrap_or(0) as usize
        };
        self.buf.get(start..end).unwrap_or(&[])
    }

    pub fn families(&self) -> usize {
        self.ends.len()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }
}

/// Precomputed HELP/TYPE headers of a resource's families, for both formats.
#[derive(Debug, Default)]
pub struct Headers {
    pub raw: Vec<String>,
    pub open_metrics: Vec<String>,
    pub text_plain: Vec<String>,
    pub metric_names: Vec<String>,
}

const HELP_PREFIX: &str = "# HELP ";
const TYPE_PREFIX: &str = "# TYPE ";

impl Headers {
    pub fn new(raw: Vec<String>) -> Self {
        let mut open_metrics = Vec::with_capacity(raw.len());
        let mut text_plain = Vec::with_capacity(raw.len());
        let mut metric_names = Vec::with_capacity(raw.len());
        for h in &raw {
            let (name, om) = parse_header_static(h, false);
            let (_, text) = parse_header_static(h, true);
            open_metrics.push(om);
            text_plain.push(text);
            metric_names.push(name);
        }
        Headers {
            raw,
            open_metrics,
            text_plain,
            metric_names,
        }
    }

    pub fn len(&self) -> usize {
        self.raw.len()
    }

    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }
}

fn with_newline(mut s: String) -> String {
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// `parseHeaderStatic`: the metric name of a header, and the header rewritten
/// for the format (text format turns `info`/`stateset` TYPE lines into `gauge`).
fn parse_header_static(header: &str, is_text_plain: bool) -> (String, String) {
    let mut rest = header;
    let mut metric_name = String::new();
    if rest.len() > HELP_PREFIX.len() && rest.starts_with(HELP_PREFIX) {
        rest = &rest[HELP_PREFIX.len()..];
        if let Some(space) = rest.find(' ')
            && space > 0
        {
            metric_name = rest[..space].to_owned();
        }
        match rest.find('\n') {
            None => return (metric_name, with_newline(header.to_owned())),
            Some(nl) => rest = &rest[nl + 1..],
        }
    }
    let mut needs_modification = false;
    let mut type_last_space: Option<usize> = None;
    if rest.len() > TYPE_PREFIX.len() && rest.starts_with(TYPE_PREFIX) {
        rest = &rest[TYPE_PREFIX.len()..];
        if let Some(space) = rest.find(' ')
            && space > 0
        {
            type_last_space = Some(header.len() - rest.len() + space);
            if is_text_plain {
                let suffix = &rest[space + 1..];
                let suffix = suffix.strip_suffix('\n').unwrap_or(suffix);
                needs_modification = suffix == "info" || suffix == "stateset";
            }
        }
    }
    let rewritten = match (needs_modification, type_last_space) {
        (true, Some(ls)) => format!("{}gauge\n", &header[..=ls]),
        _ => header.to_owned(),
    };
    (metric_name, with_newline(rewritten))
}

/// Called for every add or update of an object a full store drops.
pub type OnDropped = Box<dyn Fn() + Send + Sync>;

/// The metrics of one resource (in one namespace), by object UID.
pub struct MetricsStore {
    metrics: RwLock<HashMap<String, Arc<Rendered>>>,
    last_resource_version: Mutex<String>,
    object_limit: i64,
    object_count: Mutex<i64>,
    on_dropped: Option<OnDropped>,
}

impl Default for MetricsStore {
    fn default() -> Self {
        Self::new(0, None)
    }
}

impl MetricsStore {
    /// A store keeping at most `object_limit` objects (0: no limit).
    pub fn new(object_limit: i64, on_dropped: Option<OnDropped>) -> Self {
        MetricsStore {
            metrics: RwLock::new(HashMap::new()),
            last_resource_version: Mutex::new(String::new()),
            object_limit,
            object_count: Mutex::new(0),
            on_dropped,
        }
    }

    /// Adds or updates an object; `render` runs only if the object is kept.
    pub fn add(&self, uid: &str, resource_version: &str, render: impl FnOnce() -> Rendered) {
        self.set_last_resource_version(resource_version);
        if self.object_limit > 0 && !self.metrics.read().contains_key(uid) {
            let mut count = self.object_count.lock();
            if *count >= self.object_limit {
                if let Some(cb) = &self.on_dropped {
                    cb();
                }
                return;
            }
            *count += 1;
        }
        let rendered = Arc::new(render());
        self.metrics.write().insert(uid.to_owned(), rendered);
    }

    /// Removes an object.
    pub fn delete(&self, uid: &str, resource_version: &str) {
        self.set_last_resource_version(resource_version);
        let removed = self.metrics.write().remove(uid).is_some();
        if removed && self.object_limit > 0 {
            *self.object_count.lock() -= 1;
        }
    }

    /// Replaces the contents with `items` (`(uid, resource version, object)`):
    /// objects missing from the list are deleted first, then every listed one
    /// is upserted, so a concurrent scrape never sees an empty store.
    pub fn replace<O>(
        &self,
        items: &[(String, String, O)],
        resource_version: &str,
        render: impl Fn(&O) -> Rendered,
    ) {
        let listed: HashSet<&str> = items.iter().map(|(uid, _, _)| uid.as_str()).collect();
        let kept = {
            let mut m = self.metrics.write();
            m.retain(|uid, _| listed.contains(uid.as_str()));
            m.len() as i64
        };
        if self.object_limit > 0 {
            *self.object_count.lock() = kept;
        }
        for (uid, rv, obj) in items {
            self.add(uid, rv, || render(obj));
        }
        self.set_last_resource_version(resource_version);
    }

    /// [`replace`](Self::replace) with metrics rendered while the list was
    /// buffered (Go: the store's `Transformer` during a WatchList sync).
    pub fn replace_rendered(&self, items: Vec<(String, String, Rendered)>, resource_version: &str) {
        let listed: HashSet<&str> = items.iter().map(|(uid, _, _)| uid.as_str()).collect();
        let kept = {
            let mut m = self.metrics.write();
            m.retain(|uid, _| listed.contains(uid.as_str()));
            m.len() as i64
        };
        drop(listed);
        if self.object_limit > 0 {
            *self.object_count.lock() = kept;
        }
        for (uid, rv, rendered) in items {
            self.add(&uid, &rv, || rendered);
        }
        self.set_last_resource_version(resource_version);
    }

    pub fn last_resource_version(&self) -> String {
        self.last_resource_version.lock().clone()
    }

    fn set_last_resource_version(&self, rv: &str) {
        let mut l = self.last_resource_version.lock();
        if *l != rv {
            rv.clone_into(&mut l);
        }
    }

    pub fn len(&self) -> usize {
        self.metrics.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.metrics.read().is_empty()
    }

    fn snapshot_into(&self, out: &mut Vec<Arc<Rendered>>) {
        out.extend(self.metrics.read().values().cloned());
    }
}

/// The stores of one resource (one per namespace) and their shared headers.
pub struct MetricsWriter {
    pub resource_name: String,
    pub headers: Arc<Headers>,
    pub stores: Vec<Arc<MetricsStore>>,
}

impl MetricsWriter {
    pub fn new(
        resource_name: impl Into<String>,
        headers: Arc<Headers>,
        stores: Vec<Arc<MetricsStore>>,
    ) -> Self {
        MetricsWriter {
            resource_name: resource_name.into(),
            headers,
            stores,
        }
    }

    /// Writes every family: its header (unless blanked), then every object's
    /// samples of it. Writes nothing when no store holds an object.
    pub fn write_all(&self, headers: &[&str], out: &mut Vec<u8>) {
        let _ =
            self.write_all_chunked(headers, out, usize::MAX, &mut |_| ControlFlow::Continue(()));
    }

    /// [`write_all`](Self::write_all), calling `flush` whenever `out` holds
    /// at least `chunk` bytes, so callers can stream large outputs. `flush`
    /// returns `Break` to stop early (the reader went away).
    pub fn write_all_chunked(
        &self,
        headers: &[&str],
        out: &mut Vec<u8>,
        chunk: usize,
        flush: &mut dyn FnMut(&mut Vec<u8>) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        if self.stores.is_empty() {
            return ControlFlow::Continue(());
        }
        let mut objects = Vec::new();
        for s in &self.stores {
            s.snapshot_into(&mut objects);
        }
        if objects.is_empty() {
            return ControlFlow::Continue(());
        }
        for (i, help) in headers.iter().enumerate() {
            if !help.is_empty() {
                out.extend_from_slice(help.as_bytes());
                if !help.ends_with('\n') {
                    out.push(b'\n');
                }
            }
            for o in &objects {
                out.extend_from_slice(o.family(i));
                if out.len() >= chunk {
                    flush(out)?;
                }
            }
        }
        ControlFlow::Continue(())
    }
}

/// `SanitizeHeaders`: picks the headers of each writer for the format and
/// blanks every header whose metric name an earlier writer already declared.
pub fn sanitize_headers<'a>(
    is_text_plain: bool,
    writers: &[&'a MetricsWriter],
) -> Vec<Vec<&'a str>> {
    let mut seen: HashSet<&str> = HashSet::new();
    writers
        .iter()
        .map(|w| {
            let src = if is_text_plain {
                &w.headers.text_plain
            } else {
                &w.headers.open_metrics
            };
            src.iter()
                .zip(&w.headers.metric_names)
                .map(|(h, name)| {
                    if h.is_empty() {
                        return "";
                    }
                    if !name.is_empty() && !seen.insert(name.as_str()) {
                        return "";
                    }
                    h.as_str()
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Rendered {
        Rendered::new(s.as_bytes().to_vec(), vec![s.len() as u32])
    }

    #[test]
    fn header_rewrite() {
        let h = Headers::new(vec![
            "# HELP kube_a A.\n# TYPE kube_a info\n".into(),
            "# HELP kube_b B.\n# TYPE kube_b stateset".into(),
            "# HELP kube_c C.\n# TYPE kube_c gauge\n".into(),
            String::new(),
        ]);
        assert_eq!(h.text_plain[0], "# HELP kube_a A.\n# TYPE kube_a gauge\n");
        assert_eq!(h.open_metrics[0], "# HELP kube_a A.\n# TYPE kube_a info\n");
        assert_eq!(h.text_plain[1], "# HELP kube_b B.\n# TYPE kube_b gauge\n");
        assert_eq!(
            h.open_metrics[1],
            "# HELP kube_b B.\n# TYPE kube_b stateset\n"
        );
        assert_eq!(h.metric_names, vec!["kube_a", "kube_b", "kube_c", ""]);
        assert_eq!(h.text_plain[3], "");
    }

    #[test]
    fn store_limit_and_replace() {
        let s = MetricsStore::new(2, None);
        s.add("a", "1", || r("a 1\n"));
        s.add("b", "2", || r("b 1\n"));
        s.add("c", "3", || r("c 1\n"));
        assert_eq!(s.len(), 2);
        s.add("a", "4", || r("a 2\n"));
        assert_eq!(s.len(), 2);
        s.delete("b", "5");
        s.add("c", "6", || r("c 1\n"));
        assert_eq!(s.len(), 2);
        let items = vec![("x".to_owned(), "7".to_owned(), "x 1\n")];
        s.replace(&items, "9", |o| r(o));
        assert_eq!(s.len(), 1);
        assert_eq!(s.last_resource_version(), "9");
    }

    #[test]
    fn writer_and_dedup() {
        let headers = Arc::new(Headers::new(vec!["# HELP m M.\n# TYPE m gauge\n".into()]));
        let st = Arc::new(MetricsStore::default());
        let w1 = MetricsWriter::new("a", headers.clone(), vec![st.clone()]);
        let w2 = MetricsWriter::new("b", headers, vec![Arc::new(MetricsStore::default())]);
        let san = sanitize_headers(true, &[&w1, &w2]);
        assert_eq!(san[1], vec![""]);
        let mut out = Vec::new();
        w1.write_all(&san[0], &mut out);
        assert!(out.is_empty(), "no objects: no header");
        st.add("u", "1", || r("m 1\n"));
        w1.write_all(&san[0], &mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "# HELP m M.\n# TYPE m gauge\nm 1\n"
        );
    }
}
