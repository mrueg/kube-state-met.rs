# client-go Reflector vs kube-runtime watcher

Sources were read from local checkouts on 2026-11-26. Path prefixes used below:

- `CG` = `$(go env GOMODCACHE)/k8s.io/client-go@v0.37.2`
- `AM` = `$(go env GOMODCACHE)/k8s.io/apimachinery@v0.37.2`
- `KR` / `KC` / `KCO` = `~/.cargo/registry/src/index.crates.io-*/kube-{runtime,client,core}-5.2.0`
- `KO` = `~/.cargo/registry/src/index.crates.io-*/k8s-openapi-0.29.0`
- `KSM` = `/home/mrueg/repos/kube-state-metrics`

## 1. client-go `cache.Reflector` (v0.37.2)

**How KSM wires it.** KSM calls `cache.NewReflectorWithOptions(sharding.NewShardedListWatch(..., instrumentedListWatch), expectedType, store, {ResyncPeriod: 0})` and passes `*MetricsStore` as the store (KSM `internal/store/builder.go:774-791`). It creates one reflector per configured namespace (`builder.go:668`, `:728`). Each per-resource `ListWatch` overwrites `opts.FieldSelector` and calls the typed client (for example `internal/store/secret.go:221-233`).

**Store contract.** The reflector only needs `Add`, `Update`, `Delete`, `Replace(list, rv)` and `Resync` (`CG tools/cache/reflector.go:71-90`). Two interfaces are optional:
- `ReflectorBookmarkStore.Bookmark(rv) error` (`:94-96`). The reflector calls it on every BOOKMARK (`:1064-1070`).
- `TransformingStore.Transformer()` (`:100-103`). It is used only for the WatchList temporary store (`:831-836`). That store runs the transform on Add/Update (`CG tools/cache/store.go:255-268`).

**Run loop.** `RunWithContext` runs `ListAndWatchWithContext` inside `delayHandler.Until(ctx, immediate=true, sliding=true)` (`reflector.go:428-433`). The backoff is 800 ms initial, factor 2, jitter 1.0, capped at 30 s, and resets after 2 min without backoff (`:57-67`, `:317-326`). Every return from ListAndWatch, including a 410 relist, therefore waits one backoff step.

**List path (WatchList off or fallback)** (`:674-783`):
- **Resource version.** `relistResourceVersion()` returns `"0"` on the first list, then `lastSyncResourceVersion`, or `""` after an Expired or TooLarge error (`:1116-1132`).
- **Page size.** The pager defaults to `PageSize=500` (`CG tools/pager/pager.go:31,61-65`). It is set to 0 (no `limit` at all) when `RV != "" && RV != "0"`, unless `WatchListPageSize` is set or an earlier RV=0 list came back paginated (`reflector.go:696-717`, `:760-762`).
- **Paging.** The pager sends `limit` and follows `continue`. It clears `resourceVersion` and `resourceVersionMatch` on continuation pages. If a continuation page returns Expired, it falls back to one full list with no limit (`pager.go:92-167`).
- **Expired/TooLarge on the list.** The reflector sets the "unavailable" flag and immediately re-lists with RV="" (`reflector.go:719-729`).
- **Replace.** The result goes to `store.Replace(items, listRV)` (`:776`, `:910-917`).

**Watch path** (`:561-670`):
- **Options.** `ResourceVersion=LastSyncResourceVersion()`, `TimeoutSeconds` random in [300,600] s, `AllowWatchBookmarks=true` (`:588-598`).
- **Event mapping.** ADDED→`Add`, MODIFIED→`Update`, DELETED→`Delete`, BOOKMARK→optional `Bookmark` plus an RV update, ERROR→`apierrors.FromObject` (`:1010-1073`).
- **Normal close.** A clean close (EOF or timeout) restarts the watch immediately from the last RV with no relist, because the loop continues (`:642-669`).
- **Expired (410) or any other error.** The watch returns nil and the outer loop re-lists after a backoff step. The re-list uses the last seen RV, not "" (`:647-651`, `:664-667`, and `DefaultWatchErrorHandler` `:215-229`).
- **Retries without relist.** Connection-refused and 429 back off and re-watch (`:1195-1205`, `:652-659`). Internal errors (500) retry within `MaxInternalErrorRetryDuration` (`:660-662`).
- **Very short watch.** A watch that closes in under 1 s with zero events is a `VeryShortWatchError` (`:1089-1092`).
- **Expired detection.** Expired means `reason: Expired` or `reason: Gone` (`:1154-1160`). TooLarge means status cause `ResourceVersionTooLarge` (`:1162-1191`).
- **Events with the wrong type are skipped.** Objects whose Go type differs from expected are skipped. For an `unstructured` expected type (custom resources), events whose GVK differs from the expected GVK are also skipped (`:1013-1024`).

**WatchList / sendInitialEvents.**
- **Default.** `WatchListClient` is Beta and **default true since 1.35** (`CG features/known_features.go:103,141-144`). It can be overridden with the env var `KUBE_FEATURE_WatchListClient=false` (`CG features/envvar.go:169`). The reflector enables it unless the ListerWatcher reports `IsWatchListSemanticsUnSupported()==true` (`reflector.go:361-368`, `CG util/watchlist/watch_list.go:96-102`).
- **KSM opt-out.** KSM's `InstrumentedListerWatcher` returns true when `--object-limit>0` (`KSM pkg/watch/watch.go:105-114`). The sharding wrapper delegates to it (`KSM pkg/sharding/listwatch.go:196-203`). So **KSM uses WatchList by default** and falls back to List+Watch only when an object limit is set.
- **Request.** `watchList()` sends a watch with `ResourceVersion=rewatchResourceVersion()` (`""` initially and after an Expired error, otherwise the last RV), `AllowWatchBookmarks`, `SendInitialEvents=true`, `ResourceVersionMatch=NotOlderThan` and a random timeout (`reflector.go:847-860`, `:1135-1144`).
- **Collecting the initial state.** Events go into a temporary `Store` (with the transformer) until a BOOKMARK carrying annotation `k8s.io/initial-events-end: "true"` arrives (`:1058-1063`, `AM pkg/apis/meta/v1/types.go:508`). The reflector then calls `store.Replace(temp.List(), bookmarkRV)` and **keeps using the same watch stream** (`:887-907`, `:1077-1082`).
- **Fallback.** Any non-retriable watchList error makes `ListAndWatch` fall back to a regular list for that cycle (`:483-505`). A 422 from a server whose WatchList feature is off is such an error. Expired or TooLarge instead retries watchList with RV="" (`:820-827`). Connection-refused or 429 back off and retry (`:815-818`). The next cycle tries watchList again.

## 2. kube-runtime `watcher` (kube 5.2.0)

**Events** (`KR src/watcher.rs:65-100`):
- `Init`: a (re)list is starting.
- `InitApply(K)`: one object of the initial set.
- `InitDone`: the initial set is complete. Objects that are not in it must be treated as deleted.
- `Apply(K)`: an ADDED or MODIFIED watch event.
- `Delete(K)`: a DELETED watch event.

Bookmarks are never surfaced. They only advance the internal RV (`:742-745`).

**Config** (`:249-318`). Defaults: `bookmarks=true`, `page_size=Some(500)`, `timeout=None` (sent as 290), `list_semantic=MostRecent`, `initial_list_strategy=ListWatch`. Builders: `.timeout()`, `.fields()`, `.labels()`, `.labels_from()`, `.any_semantic()`, `.disable_bookmarks()`, `.page_size()`, `.streaming_lists()` (`:335-427`). `page_size` and `list_semantic` only apply to the ListWatch strategy.

**State machine** (`:563-777`):
- **Empty, ListWatch strategy.** Emits `Init`, then lists pages, emitting one `InitApply` per item, then `InitDone`, then moves to InitListed(rv) (`:573-644`). A list error emits `Err(InitialListFailed)`, and the next poll **starts over with a new `Init`** (`:635-642`). A list whose RV is empty and that has no continue token is `Err(NoResourceVersion)` (`:624-626`).
- **Empty, StreamingList strategy.** Watches with `resourceVersion=0` and `sendInitialEvents=true`, then emits `Init` (`:579-597`). ADDED/MODIFIED become `InitApply`. DELETED is logged and dropped. A BOOKMARK carrying `k8s.io/initial-events-end` (any value) becomes `InitDone` (`:645-691`). **There is no fallback to list.** A rejected streaming watch returns `WatchStartFailed` and retries StreamingList forever (`:587-594`).
- **InitListed(rv).** Watches from `rv` with no sendInitialEvents (`:692-715`). If the watch request itself fails, including an HTTP 410, it stays in InitListed and retries from the **same rv** (`:703-712`). Only a 410 delivered as an ERROR *event* resets the watcher to Empty, which is a full relist (`:746-761`, `:667-679`).
- **Watching.** ADDED/MODIFIED→`Apply`, DELETED→`Delete`. The RV is taken from each object. An object without an RV is `NoResourceVersion` and triggers a relist (`:720-741`). When the stream ends, the watcher goes back to InitListed: a re-watch, not a relist (`:774`).
- **Idle timeout.** Each poll is wrapped in an idle timeout of `timeout.unwrap_or(290)+5` s. `timeout(0)` disables it (`:516-556`).

**Backoff.** `watcher()` itself retries immediately. You must add `.default_backoff()`: 800 ms→30 s, factor 2, jitter, reset after 120 s (`:1045-1052`). `StreamBackoff` **resets on any `Ok` item** (`KR src/utils/stream_backoff.rs:72-93`).

**Custom resources.** Build an `ApiResource` with `ApiResource::from_gvk_with_plural(&gvk, plural)`; `from_gvk` guesses the plural (`KCO src/discovery/mod.rs:39-58`). Then pass `Api::<DynamicObject>::all_with(client, &ar)` or `namespaced_with(client, ns, &ar)` (`KC src/api/mod.rs:97,113`) to `watcher(api, cfg)`. The path is `/{api|apis}/{group}/{version}/[namespaces/{ns}/]{plural}` (`KCO src/resource.rs:93-109`).

**Per-namespace watchers and cancellation.** To mirror KSM's per-namespace reflectors, build one `Api::namespaced_with` per namespace. `Client` is cheap to clone. Spawn one task per namespace that drives `watcher(...).default_backoff()`. The watcher is a lazy `unfold` stream (`KR src/watcher.rs:856-868`), so dropping it or aborting the task cancels the in-flight HTTP request. For per-GVK stop (the equivalent of KSM's `GetGVKStopChan`, `builder.go:785-787`), use a `tokio_util::sync::CancellationToken` per GVK with `stream.take_until(token.cancelled())`, or keep the task's `AbortHandle`.

## 3. Wire protocol the fake API server must accept

**Query parameters** are encoded from `ListOptions` json tags with `omitempty` (`AM pkg/conversion/queryparams/convert.go:113-118`, `AM pkg/apis/meta/v1/types.go:367-467`). The Go client sorts the query (`url.Values.Encode`). kube emits it in insertion order. **Parse the query; never compare it as a string.**

| Scenario | client-go (KSM typed client) | kube watcher |
|---|---|---|
| Initial list | `GET /api/v1/[namespaces/NS/]pods?limit=500&resourceVersion=0[&fieldSelector=..]` | `?limit=500[&fieldSelector..][&labelSelector..]`, with **no RV** (MostRecent) |
| Next page | `?limit=500&continue=TOKEN` (RV cleared) | `?limit=500&continue=TOKEN` |
| Relist after 410 | `?resourceVersion=<lastRV>` (**no limit**); after Expired: `?limit=500` | same as the initial list |
| Watch | `?watch=true&resourceVersion=RV&allowWatchBookmarks=true&timeoutSeconds=N&timeout=<N>s-ish` | `?watch=true&timeoutSeconds=290&allowWatchBookmarks=true&resourceVersion=RV` |
| WatchList | `?watch=true&sendInitialEvents=true&resourceVersionMatch=NotOlderThan&allowWatchBookmarks=true&timeoutSeconds=N&timeout=..` (RV omitted at first) | `...&sendInitialEvents=true&resourceVersionMatch=NotOlderThan&resourceVersion=0` |

Where each row comes from:
- client-go typed Watch sets `watch=true` and also `timeout=<Go duration>` (for example `7m3s`) next to `timeoutSeconds` (`CG gentype/type.go:189-201`, `CG rest/request.go:577-580`).
- kube list: `KCO src/params.rs:97-124`. kube watch: `KCO src/params.rs:379-398`, `KCO src/request.rs:70-77`. kube's `any_semantic()` only sends `resourceVersion=0&resourceVersionMatch=NotOlderThan` when `limit` is unset (`params.rs:110-112`).

**Headers.**
- **client-go typed clients request protobuf.** They send `Accept: application/vnd.kubernetes.protobuf,application/json` (`CG gentype/type.go:178,196`, `CG rest/request.go:189-203`), and KSM forces the same (`KSM pkg/util/utils.go:51-52`). Responses are decoded according to the response `Content-Type` (`CG rest/request.go:834-839`). **Answering `application/json` is enough**, but the server must not 406.
- client-go's dynamic client (used for custom resources) uses JSON (`CG dynamic/simple.go:46-47`).
- kube sends no `Accept` on list or watch (`KCO src/request.rs:59-77`). On watch it sends `Accept-Encoding: identity` (`KC src/client/mod.rs:355-358`).

**List body.**

```json
{"kind":"PodList","apiVersion":"v1","metadata":{"resourceVersion":"42","continue":"<tok or omit>"},"items":[...]}
```

- kube requires a non-empty `metadata.resourceVersion` on the last page (`KR watcher.rs:622-626`). kube defaults a missing `kind`/`apiVersion` (`KCO src/object.rs:42-51`).
- client-go unstructured lists **require** a top-level `kind`. Items without kind or apiVersion inherit them from the list kind minus "List" (`AM pkg/apis/meta/v1/unstructured/helpers.go:476-512`).
- k8s-openapi accepts items without kind or apiVersion, but **rejects mismatching** ones (`KO src/v1_37/api/core/v1/pod.rs:103-114`).

**Watch body.** Status 200, `Content-Type: application/json`, one event per line terminated by `\n`. kube frames with `LinesCodec` (`KC src/client/mod.rs:364-371`):

```json
{"type":"ADDED","object":{"kind":"Pod","apiVersion":"v1","metadata":{...,"resourceVersion":"43"}}}
{"type":"BOOKMARK","object":{"kind":"Pod","apiVersion":"v1","metadata":{"resourceVersion":"50","annotations":{"k8s.io/initial-events-end":"true"}}}}
{"type":"ERROR","object":{"kind":"Status","apiVersion":"v1","metadata":{},"status":"Failure","message":"too old resource version: 1 (50)","reason":"Expired","code":410}}
```

- BOOKMARK **must carry kind and apiVersion**. kube's `Bookmark` flattens a `TypeMeta` with required fields (`KCO src/watch.rs:46-67`, `KCO src/metadata.rs:10-18`). client-go drops GVK-mismatched custom resource events (`reflector.go:1019-1024`).
- Send the initial-events-end bookmark **only** when `sendInitialEvents=true`, after all synthetic ADDED events.

**Unsupported parameter.** Return a non-2xx JSON `Status`, which both clients parse (`KC src/client/mod.rs:585-611`). Mirror the real apiserver's 422 Invalid: `{"kind":"Status","apiVersion":"v1","status":"Failure","reason":"Invalid","code":422,"message":"... sendInitialEvents is forbidden for watch unless the WatchList feature gate is enabled"}`. The rules are in `AM pkg/apis/meta/internalversion/validation/validation.go:28-80`: `resourceVersionMatch` on a watch requires `sendInitialEvents`, `sendInitialEvents` on a list is forbidden, and `Exact` together with RV "0" is forbidden.

## 4. Mapping watcher events onto a MetricsStore

KSM `MetricsStore` (`KSM pkg/metrics_store/metrics_store.go`):
- It is keyed by **UID**. `Add` renders families and stores them (`:127-164`). `Update` is the same as `Add` (`:231-234`).
- `Delete` removes the UID (`:237-251`).
- `Replace` first deletes UIDs that are not in the list, then upserts, and never empties the map, so concurrent scrapes stay consistent (`:281-315`).
- `Transformer` pre-renders objects in the WatchList temporary store to cut peak memory (`:332-343`).

Recommended Rust driver:
- `Init` → start a new `HashMap<Uid, Rendered>` buffer and drop any previous partial one. kube re-emits `Init` after every failed list (`KR watcher.rs:635-642`).
- `InitApply(obj)` → render the metrics immediately and buffer `(uid, families)`. This is the equivalent of `Transformer`: it avoids holding full objects.
- `InitDone` → `store.replace(buffer)`, keeping KSM's delete-missing-then-upsert order. This matches kube's own reflector, which buffers and swaps on `InitDone` (`KR src/reflector/store.rs:160-183`).
- `Apply(obj)` → `store.add(obj)` (upsert by UID). `Delete(obj)` → `store.delete(uid)`.
- `Err(e)` → log, increment the list/watch error metrics, and continue. Never use `try_for_each`, which ends on the first `Err` (`KR watcher.rs:802-805`).
- `--object-limit`: enforce it in `add`, as KSM does, and in `InitApply` buffering. It is applied per page in KSM's List (`KSM pkg/watch/watch.go:127-164`).
- `--use-apiserver-cache`: KSM forces `resourceVersion=0` on every List (`KSM pkg/watch/watch.go:122-124`, applied before the inner List). With kube this needs `any_semantic()` **and** no page size: set the public field `cfg.page_size = None`.

## Semantic traps

1. **WatchList is on by default in client-go 0.37** (`features/known_features.go:141-144`). KSM's real traffic is a `sendInitialEvents` watch, not LIST, unless `--object-limit` is set. The fake server must implement the initial-events-end bookmark, or answer 422 so client-go falls back (`reflector.go:483-505`).
2. **A server that silently ignores `sendInitialEvents` hangs client-go.** client-go loops in `watchList` waiting for the bookmark (`reflector.go:870-889`). It only falls back if the watch errors or closes in under 1 s with no events (`:1089-1092`).
3. **kube StreamingList has no fallback** (`watcher.rs:587-594`). It starts at `resourceVersion=0`, while client-go starts with RV unset (consistent read). Keep the default ListWatch strategy unless every target cluster is known to serve WatchList. I did not verify the server-side default version against a primary source.
4. **`any_semantic()` + `page_size=500` does not send `resourceVersion=0`.** It silently becomes a quorum list (`KCO params.rs:110-112`).
5. **kube only relists on a 410 delivered as a watch ERROR event.** An HTTP 410 on the watch request loops in InitListed with the same RV (`watcher.rs:703-712`). The fake server must send the 410 as a 200 stream carrying an ERROR event.
6. **`StreamBackoff` resets on every `Ok`.** With ListWatch, every retry emits `Ok(Init)` before the list error, so persistent list failures retry at about 0.8 s forever instead of growing to 30 s (`stream_backoff.rs:88-91`, `watcher.rs:573-578`). Throttle failing lists manually, for example with a backoff on the `Err` that resets only on `InitDone`.
7. **client-go waits a backoff step before every relist** (`delayHandler.Until`, `reflector.go:428-433`). After a 410 it relists at the last RV with **no limit** (`:703-716`). kube relists with `limit=500` and no RV. Tests must not assert identical request sequences.
8. **KSM's `MetricsStore.Bookmark(string)` has no `error` return**, so it does not implement `ReflectorBookmarkStore` (`reflector.go:94-96` vs `metrics_store.go:353-355`). client-go never calls it. kube does not surface bookmarks either, so nothing is lost. `LastStoreSyncResourceVersion` is only used in tests.
9. **BOOKMARK and custom-resource events need correct `kind`/`apiVersion`.** Without them kube fails to deserialize and client-go silently drops custom-resource events (`watch.rs:46-67`, `reflector.go:1019-1024`). Typed k8s-openapi objects with a *wrong* kind are hard errors (`pod.rs:103-114`).
10. **Watch timeouts differ.** client-go uses a random 300–600 s timeout and sends `timeout=` plus `timeoutSeconds=`. kube sends 290 s and applies a client idle timeout of +5 s. kube treats a silent stream as dead after 295 s, so a fake server that holds idle watches open longer will see kube reconnect.
11. **kube emits `Init` on every relist, including retries.** Discard the partial buffer on each `Init`. Never apply `InitApply` events directly to the live store, or deleted objects survive a relist.
