//! The list and watch of a store with `--object-limit` (Go:
//! `InstrumentedListerWatcher` in `pkg/watch/watch.go`). The list reads one
//! page of at most `limit` objects and drops its continue token, so the rest
//! of the collection is never fetched; the watch then starts from that page's
//! resourceVersion. kube-runtime's watcher would follow every page, so this
//! stream stands in for it and yields the same events.

use std::collections::VecDeque;
use std::fmt::Debug;

use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use kube::api::{Api, WatchEvent, WatchParams};
use kube::core::ObjectList;
use kube::runtime::watcher::{self, Event};
use kube::{Resource, ResourceExt};
use serde::de::DeserializeOwned;

use crate::telemetry::ListWatchMetrics;

pub struct Limited {
    pub limit: u32,
    pub fields: Option<String>,
    pub use_api_server_cache: bool,
    pub metrics: ListWatchMetrics,
    pub go_type: &'static str,
}

enum Phase<K> {
    List,
    Emit(VecDeque<Event<K>>, String),
    Watch(String),
    Watching(String, BoxStream<'static, kube::Result<WatchEvent<K>>>),
}

/// List one page, then watch from its resourceVersion; relist when the
/// resourceVersion is too old. Errors are yielded and the caller backs off.
pub fn stream<K>(api: Api<K>, cfg: Limited) -> impl Stream<Item = watcher::Result<Event<K>>>
where
    K: Resource + Clone + DeserializeOwned + Debug + Send + 'static,
{
    futures::stream::unfold(
        (api, cfg, Phase::List),
        |(api, cfg, mut phase)| async move {
            loop {
                match phase {
                    Phase::List => match list_page(&api, &cfg).await {
                        Ok(list) => {
                            let rv = list.metadata.resource_version.unwrap_or_default();
                            let mut events: VecDeque<Event<K>> =
                                list.items.into_iter().map(Event::InitApply).collect();
                            events.push_front(Event::Init);
                            events.push_back(Event::InitDone);
                            phase = Phase::Emit(events, rv);
                        }
                        Err(e) => {
                            let e = watcher::Error::InitialListFailed(e);
                            return Some((Err(e), (api, cfg, Phase::List)));
                        }
                    },
                    Phase::Emit(mut events, rv) => match events.pop_front() {
                        Some(ev) => return Some((Ok(ev), (api, cfg, Phase::Emit(events, rv)))),
                        None => phase = Phase::Watch(rv),
                    },
                    Phase::Watch(rv) => {
                        let mut wp = WatchParams::default();
                        if let Some(f) = &cfg.fields {
                            wp = wp.fields(f);
                        }
                        match api.watch(&wp, &rv).await {
                            Ok(s) => {
                                cfg.metrics.watch_total.inc(&["success", cfg.go_type]);
                                phase = Phase::Watching(rv, s.boxed());
                            }
                            Err(e) => {
                                cfg.metrics.watch_total.inc(&["error", cfg.go_type]);
                                let e = watcher::Error::WatchStartFailed(e);
                                return Some((Err(e), (api, cfg, Phase::Watch(rv))));
                            }
                        }
                    }
                    Phase::Watching(mut rv, mut s) => match s.next().await {
                        Some(Ok(WatchEvent::Added(o) | WatchEvent::Modified(o))) => {
                            rv = o.resource_version().unwrap_or(rv);
                            return Some((Ok(Event::Apply(o)), (api, cfg, Phase::Watching(rv, s))));
                        }
                        Some(Ok(WatchEvent::Deleted(o))) => {
                            rv = o.resource_version().unwrap_or(rv);
                            return Some((
                                Ok(Event::Delete(o)),
                                (api, cfg, Phase::Watching(rv, s)),
                            ));
                        }
                        Some(Ok(WatchEvent::Bookmark(b))) => {
                            phase = Phase::Watching(b.metadata.resource_version, s);
                        }
                        Some(Ok(WatchEvent::Error(status))) => {
                            // 410 Gone: the resourceVersion is too old, list again.
                            let next = if status.code == 410 {
                                Phase::List
                            } else {
                                Phase::Watch(rv)
                            };
                            let e = watcher::Error::WatchError(status);
                            return Some((Err(e), (api, cfg, next)));
                        }
                        Some(Err(e)) => {
                            let e = watcher::Error::WatchFailed(e);
                            return Some((Err(e), (api, cfg, Phase::Watch(rv))));
                        }
                        // The server ended the watch (timeout): watch again.
                        None => phase = Phase::Watch(rv),
                    },
                }
            }
        },
    )
}

/// One page of the list, cut to the limit (Go: `List` with `Limit`, the items
/// past it dropped and the continue token cleared). The query is built by
/// hand: kube's `ListParams` drops `resourceVersion=0` when a limit is set,
/// while Go sends both.
async fn list_page<K>(api: &Api<K>, cfg: &Limited) -> kube::Result<ObjectList<K>>
where
    K: Resource + Clone + DeserializeOwned + Debug,
{
    let query = {
        let mut qp = form_urlencoded::Serializer::new(String::new());
        if let Some(f) = &cfg.fields {
            qp.append_pair("fieldSelector", f);
        }
        qp.append_pair("limit", &cfg.limit.to_string());
        if cfg.use_api_server_cache {
            qp.append_pair("resourceVersion", "0");
        }
        qp.finish()
    };
    let url = format!("{}?{query}", api.resource_url());
    let req = http::Request::get(url)
        .body(Vec::new())
        .map_err(kube::Error::HttpError)?;
    let res = api
        .clone()
        .into_client()
        .request::<ObjectList<K>>(req)
        .await;
    let mut list = match res {
        Ok(l) => {
            cfg.metrics.list_total.inc(&["success", cfg.go_type]);
            l
        }
        Err(e) => {
            cfg.metrics.list_total.inc(&["error", cfg.go_type]);
            return Err(e);
        }
    };
    list.items.truncate(cfg.limit as usize);
    cfg.metrics
        .list_objects
        .set(&[cfg.go_type], list.items.len() as f64);
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::Registry;
    use axum::extract::{RawQuery, State};
    use k8s_openapi::api::core::v1::Pod;
    use std::sync::{Arc, Mutex};

    type Queries = Arc<Mutex<Vec<String>>>;

    async fn pods(State(q): State<Queries>, RawQuery(query): RawQuery) -> String {
        let query = query.unwrap_or_default();
        q.lock().unwrap().push(query.clone());
        let pod = |n: &str| {
            format!(
                r#"{{"metadata":{{"name":"{n}","namespace":"d","uid":"{n}","resourceVersion":"5"}}}}"#
            )
        };
        if query.contains("watch=") {
            let ev = format!(r#"{{"type":"ADDED","object":{}}}"#, pod("w"));
            return ev + "\n";
        }
        format!(
            r#"{{"kind":"PodList","apiVersion":"v1","metadata":{{"resourceVersion":"10","continue":"next"}},"items":[{},{},{}]}}"#,
            pod("a"),
            pod("b"),
            pod("c")
        )
    }

    #[tokio::test]
    async fn reads_one_page_and_watches_from_it() {
        let queries = Queries::default();
        let app = axum::Router::new()
            .route("/api/v1/pods", axum::routing::get(pods))
            .with_state(Arc::clone(&queries));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        let cfg = kube::Config::new(format!("http://{addr}").parse().unwrap());
        let client = kube::Client::try_from(cfg).unwrap();
        let registry = Registry::new();
        let limited = Limited {
            limit: 2,
            fields: Some("spec.nodeName=n".into()),
            use_api_server_cache: true,
            metrics: ListWatchMetrics::new(&registry),
            go_type: "*v1.Pod",
        };
        let mut s = std::pin::pin!(stream(Api::<Pod>::all(client), limited));
        let mut names = Vec::new();
        while names.len() < 5 {
            let ev = tokio::time::timeout(std::time::Duration::from_secs(5), s.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            names.push(match ev {
                Event::Init => "init".to_owned(),
                Event::InitApply(p) => format!("list {}", p.name_any()),
                Event::InitDone => "done".to_owned(),
                Event::Apply(p) => format!("apply {}", p.name_any()),
                Event::Delete(p) => format!("delete {}", p.name_any()),
            });
        }
        assert_eq!(names, ["init", "list a", "list b", "done", "apply w"]);
        let q = queries.lock().unwrap().clone();
        // One list request, with Go's parameters; the continue token is
        // not followed.
        assert_eq!(
            q[0],
            "fieldSelector=spec.nodeName%3Dn&limit=2&resourceVersion=0"
        );
        assert!(q[1].contains("watch=true"), "{q:?}");
        assert!(q[1].contains("resourceVersion=10"), "{q:?}");
        assert!(q[1].contains("fieldSelector=spec.nodeName%3Dn"), "{q:?}");
        assert!(q.iter().all(|x| !x.contains("continue")), "{q:?}");
        let text = String::from_utf8(registry.render(&[], None)).unwrap();
        assert!(
            text.contains(
                "kube_state_metrics_list_total{resource=\"*v1.Pod\",result=\"success\"} 1"
            ),
            "{text}"
        );
        assert!(
            text.contains("kube_state_metrics_list_objects{resource=\"*v1.Pod\"} 2"),
            "{text}"
        );
    }
}
