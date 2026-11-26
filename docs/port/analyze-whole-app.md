# Rust migration plan: kube-state-metrics (Go)

1 package, 79 source files (16519 lines), 79 test files, 0 UI files, grouped into 23 units.

| Ready | Caution | Blocked | Keep in source language |
| ---: | ---: | ---: | ---: |
| 8 | 15 | 0 | 0 |

> 5 files had parse errors; their units are marked caution.

## Start here

Best first ports: safe to port (readiness) and worth porting (loops and string work, tests, size).

| # | Unit | Lines | Priority | Readiness | Tests | Bridge | Main risks |
| ---: | --- | ---: | ---: | ---: | ---: | --- | --- |
| 1 | `pkg/metric_generator/` | 125 | 72 | 69 | 46 | cgo | Pointers and manual memory (10); Callback-shaped APIs (5); Dynamic typing (1) |
| 2 | `pkg/util/proc/` | 60 | 65 | 86 | 0 | cgo | Concurrency (3); Callback-shaped APIs (2); Null references (2) |
| 3 | `pkg/options/` | 616 | 38 | 17 | 14 | cgo | Pointers and manual memory (80); String offsets and slicing (1); Null references (17) |
| 4 | `internal/store/` | 11410 | 28 | 0 | 2 | cgo | Pointers and manual memory (2151); String offsets and slicing (7); Regex features the `regex` crate lacks (2) |

### `pkg/metric_generator/`

125 lines in 2 files, wave 1, priority 72, readiness 69, bridge **cgo**.

- 1 Go modules dependencies not in the crate table: check them by hand.
- Imports `pkg/metric/`: port it first, or port them together.
- 46 test files import it directly.
- Pointers and manual memory x10: `pkg/metric_generator/generator.go:33`, `pkg/metric_generator/generator.go:44`, `pkg/metric_generator/generator.go:45`
- Callback-shaped APIs x5: `pkg/metric_generator/generator.go:33`, `pkg/metric_generator/generator.go:44`, `pkg/metric_generator/generator.go:61`
- Dynamic typing x1: `pkg/metric_generator/filter.go:21`

### `pkg/util/proc/`

60 lines in 2 files, wave 0, priority 65, readiness 86, bridge **cgo**.

- 1 Go modules dependencies not in the crate table: check them by hand.
- Leaf: imports no other unit.
- Concurrency x3: `pkg/util/proc/reaper.go:24`, `pkg/util/proc/reaper.go:36`, `pkg/util/proc/reaper.go:58`
- Callback-shaped APIs x2: `pkg/util/proc/reaper.go:36`, `pkg/util/proc/reaper.go:51`
- Null references x2: `pkg/util/proc/reaper.go:66`

### `pkg/options/`

616 lines in 4 files, wave 0, priority 38, readiness 17, bridge **cgo**.

- 4 Go modules dependencies not in the crate table: check them by hand.
- Leaf: imports no other unit.
- 14 test files import it directly.
- Pointers and manual memory x80: `pkg/options/autoload.go:116`, `pkg/options/autoload.go:123`, `pkg/options/autoload.go:138`
- String offsets and slicing x1: `pkg/options/options.go:156`
- Null references x17: `pkg/options/options.go:132`, `pkg/options/options.go:133`, `pkg/options/options.go:282`
- Callback-shaped APIs x7: `pkg/options/autoload.go:123`, `pkg/options/options.go:150`, `pkg/options/options.go:154`
- Integer width and overflow x3: `pkg/options/options.go:201`, `pkg/options/options.go:208`, `pkg/options/options.go:278`

### `internal/store/`

11410 lines in 43 files, wave 4, priority 28, readiness 0, bridge **cgo**.

- 26 Go modules dependencies not in the crate table: check them by hand.
- Imports `pkg/builder/types/`, `pkg/constant/`, `pkg/customresource/`, `pkg/metric/`, `pkg/metric_generator/`, `pkg/metrics_store/`, `pkg/options/`, `pkg/sharding/`, `pkg/util/`, `pkg/watch/`: port them first, or port them together.
- 2 test files import it directly.
- Pointers and manual memory x2151: `internal/store/builder.go:65`, `internal/store/builder.go:74`, `internal/store/builder.go:75`
- String offsets and slicing x7: `internal/store/utils.go:184`, `internal/store/utils.go:186`, `internal/store/utils.go:298`
- Regex features the `regex` crate lacks x2: `internal/store/testutils.go:23`, `internal/store/utils.go:22`
- Callback-shaped APIs x600: `internal/store/builder.go:94`, `internal/store/builder.go:257`, `internal/store/builder.go:382`
- Null references x196: `internal/store/builder.go:132`, `internal/store/builder.go:158`, `internal/store/builder.go:248`
- Integer width and overflow x186: `internal/store/builder.go:183`, `internal/store/builder.go:185`, `internal/store/builder.go:187`
- Dynamic typing x45: `internal/store/builder.go:785`, `internal/store/builder.go:797`, `internal/store/certificatesigningrequest.go:145`
- Concurrency x10: `internal/store/builder.go:26`, `internal/store/builder.go:94`, `internal/store/builder.go:743`
- Reflection and prototypes x1: `internal/store/builder.go:22`
- Division and modulo x1: `internal/store/utils.go:422`
- Exceptions x11: `internal/store/builder.go:126`, `internal/store/builder.go:145`, `internal/store/builder.go:318`

## Order by package

Packages come after the workspace packages they use. Inside a package, port by wave: a unit's imports are in earlier waves.

### 1. k8s.io/kube-state-metrics/v2 (`.`, portable, 79 test files)

| Wave | Unit | Status | Lines | Readiness | Bridge | Main risks |
| ---: | --- | --- | ---: | ---: | --- | --- |
| 0 | `pkg/constant/` | ready | 17 | 100 | cgo | - |
| 0 | `tests/lib/` | ready | 14 | 100 | cgo | - |
| 0 | `pkg/util/proc/` | ready | 60 | 86 | cgo | Concurrency (3); Callback-shaped APIs (2) |
| 0 | `pkg/metric/` | ready | 157 | 70 | cgo | Pointers and manual memory (8); String offsets and slicing (1) |
| 0 | `tests/e2e/framework/` | caution | 152 | 61 | cgo | Pointers and manual memory (19); Null references (26) |
| 0 | `pkg/watch/` | caution | 134 | 43 | cgo | Pointers and manual memory (13); String offsets and slicing (1) |
| 0 | `pkg/sharding/` | caution | 200 | 27 | cgo | Pointers and manual memory (19); Null references (13) |
| 0 | `pkg/options/` | caution | 616 | 17 | cgo | Pointers and manual memory (80); String offsets and slicing (1) |
| 1 | `pkg/metric_generator/` | ready | 125 | 69 | cgo | Pointers and manual memory (10); Callback-shaped APIs (5) |
| 1 | `pkg/metrics_store/` | caution | 418 | 8 | cgo | Pointers and manual memory (37); String offsets and slicing (12) |
| 2 | `pkg/optin/` | caution | 53 | 89 | cgo | Pointers and manual memory (4); Regex features the `regex` crate lacks (1) |
| 2 | `pkg/customresource/` | ready | 23 | 86 | cgo | Pointers and manual memory (1); Dynamic typing (1) |
| 2 | `pkg/allowdenylist/` | caution | 131 | 75 | cgo | Pointers and manual memory (11); Null references (10) |
| 3 | `pkg/builder/types/` | ready | 60 | 86 | cgo | Callback-shaped APIs (4); Dynamic typing (3) |
| 3 | `pkg/util/` | caution | 116 | 38 | cgo | Pointers and manual memory (4); Null references (25) |
| 4 | `pkg/metricshandler/` | caution | 295 | 23 | cgo | Pointers and manual memory (21); Null references (29) |
| 4 | `internal/store/` | caution | 11410 | 0 | cgo | Pointers and manual memory (2151); String offsets and slicing (7) |
| 5 | `pkg/builder/` | caution | 98 | 78 | cgo | Pointers and manual memory (23); Dynamic typing (1) |
| 5 | `internal/discovery/` | caution | 600 | 0 | cgo | Pointers and manual memory (22); String offsets and slicing (2) |
| 6 | `pkg/customresourcestate/` | caution | 975 | 0 | cgo | Pointers and manual memory (39); String offsets and slicing (3) |
| 7 | `pkg/app/` | caution | 601 | 0 | cgo | Pointers and manual memory (31); String offsets and slicing (1) |
| 8 | `internal/` | caution | 234 | 59 | cgo | Pointers and manual memory (7); Null references (21) |
| 9 | `main.go` | ready | 30 | 91 | cgo | Pointers and manual memory (1); Callback-shaped APIs (1) |

## Go modules dependencies

| Package | Units | Rust | Fit | Note |
| --- | ---: | --- | --- | --- |
| `k8s.io/klog/v2` | 11 | ? | unknown |  |
| `k8s.io/client-go/tools/cache` | 10 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/apis/meta/v1` | 6 | ? | unknown |  |
| `k8s.io/client-go/kubernetes` | 6 | ? | unknown |  |
| `k8s.io/client-go/rest` | 5 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/api/meta` | 4 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/runtime` | 4 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/watch` | 4 | ? | unknown |  |
| `github.com/prometheus/common/expfmt` | 3 | ? | unknown |  |
| `github.com/prometheus/common/version` | 3 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/apis/meta/v1/unstructured` | 3 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/runtime/schema` | 3 | ? | unknown |  |
| `github.com/prometheus/common/model` | 2 | ? | unknown |  |
| `k8s.io/api/apps/v1` | 2 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/api/resource` | 2 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/fields` | 2 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/util/validation` | 2 | ? | unknown |  |
| `k8s.io/client-go/dynamic` | 2 | ? | unknown |  |
| `k8s.io/client-go/tools/clientcmd` | 2 | ? | unknown |  |
| `k8s.io/component-base/metrics` | 2 | ? | unknown |  |
| `github.com/KimMachineGun/automemlimit/memlimit` | 1 | ? | unknown |  |
| `github.com/dgryski/go-jump` | 1 | ? | unknown |  |
| `github.com/fsnotify/fsnotify` | 1 | ? | unknown |  |
| `github.com/go-logr/logr` | 1 | ? | unknown |  |
| `github.com/gobuffalo/flect` | 1 | ? | unknown |  |
| `github.com/google/go-cmp/cmp` | 1 | ? | unknown |  |
| `github.com/netresearch/go-cron` | 1 | ? | unknown |  |
| `github.com/oklog/run` | 1 | ? | unknown |  |
| `github.com/prometheus/client_model/go` | 1 | ? | unknown |  |
| `github.com/prometheus/exporter-toolkit/web` | 1 | ? | unknown |  |
| `k8s.io/api/admissionregistration/v1` | 1 | ? | unknown |  |
| `k8s.io/api/autoscaling/v2` | 1 | ? | unknown |  |
| `k8s.io/api/batch/v1` | 1 | ? | unknown |  |
| `k8s.io/api/certificates/v1` | 1 | ? | unknown |  |
| `k8s.io/api/coordination/v1` | 1 | ? | unknown |  |
| `k8s.io/api/core/v1` | 1 | ? | unknown |  |
| `k8s.io/api/discovery/v1` | 1 | ? | unknown |  |
| `k8s.io/api/networking/v1` | 1 | ? | unknown |  |
| `k8s.io/api/policy/v1` | 1 | ? | unknown |  |
| `k8s.io/api/rbac/v1` | 1 | ? | unknown |  |
| `k8s.io/api/storage/v1` | 1 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/api/errors` | 1 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/types` | 1 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/util/intstr` | 1 | ? | unknown |  |
| `k8s.io/client-go/discovery` | 1 | ? | unknown |  |
| `k8s.io/client-go/dynamic/dynamicinformer` | 1 | ? | unknown |  |
| `k8s.io/client-go/plugin/pkg/client/auth` | 1 | ? | unknown |  |
| `k8s.io/client-go/util/watchlist` | 1 | ? | unknown |  |
| `k8s.io/component-helpers/storage/ephemeral` | 1 | ? | unknown |  |
| `k8s.io/sample-controller/pkg/apis/samplecontroller/v1alpha1` | 1 | ? | unknown |  |
| `sigs.k8s.io/controller-runtime/pkg/metrics/filters` | 1 | ? | unknown |  |
| `github.com/prometheus/client_golang/prometheus` | 8 | prometheus | good |  |
| `github.com/prometheus/client_golang/prometheus/promauto` | 3 | prometheus | good |  |
| `sigs.k8s.io/yaml/goyaml.v3` | 3 | serde_yaml_ng | good |  |
| `github.com/spf13/cobra` | 2 | clap | good |  |
| `github.com/dlclark/regexp2/v2` | 1 | fancy-regex | good |  |
| `github.com/prometheus/client_golang/prometheus/collectors` | 1 | prometheus | good |  |
| `github.com/prometheus/client_golang/prometheus/collectors/version` | 1 | prometheus | good |  |
| `github.com/prometheus/client_golang/prometheus/promhttp` | 1 | prometheus | good |  |

## Risk guide

- **String offsets and slicing** (High): Rust slices strings at UTF-8 byte offsets that fall on character boundaries. Convert offsets from the source's indexing units as needed and test with non-ASCII text.
- **Regex features the `regex` crate lacks** (High): Lookaround, backreferences and the `y` flag are not in the `regex` crate. Use `fancy-regex` for those patterns, or rewrite them, and test them on non-ASCII text: `\w`, `\b` and case-insensitive matching differ between engines even where a pattern compiles. JS `/i` without `u` never matches a non-ASCII letter to an ASCII one, Rust `(?i)` does (`ſ` matches `s`, the Kelvin sign `K` matches `k`): write `(?i-u)` or compare with `eq_ignore_ascii_case`.
- **Dynamic typing** (Medium): Values whose shape is checked at runtime need an explicit Rust enum or `serde_json::Value`.
- **Exceptions** (Low): Represent source exceptions with `Result`. Preserve the original exception type and message when errors cross the bridge.
- **Reflection and prototypes** (Medium): Proxy, Reflect, Symbol and prototype tricks need a redesign, not a translation.
- **Callback-shaped APIs** (Medium): Function-typed parameters and methods cross the bridge on every call. Keep boundaries coarse: pass data in, get data out.
- **Integer width and overflow** (Medium): Python ints never overflow; C, Go and Java ints wrap or are undefined on overflow. Pick i64, i128 or a bignum crate deliberately, and use wrapping_* where the source wraps.
- **Division and modulo** (Medium): Python `//` and `%` floor toward negative infinity; Rust `/` and `%` truncate: use `div_euclid`/`rem_euclid` or explicit floor math. C division by zero is undefined.
- **Hash iteration order** (High): Set and map iteration order comes from the source language's hash function. A port cannot reproduce it: sort before output, or make the order part of the contract.
- **Pointers and manual memory** (High): Pointer arithmetic and manual memory become slices, indices and owned types. Code that exposes pointers across the boundary needs `unsafe`, isolated and explained.
- **Concurrency** (Medium): Threads, goroutines and locks need a design in Rust (channels, Send/Sync), not a line-by-line translation; keep concurrency in the caller where possible.
- **Null references** (Medium): Any reference may be null in the source: decide per parameter whether it is `Option` and what null means, instead of mapping every value to `Option`.

## Whole app

Shape: **cli**.

| Entry point | Kind | Found from |
| --- | --- | --- |
| `main.go` | cli | `func main()` at main.go:27 |

### Every dependency

| Dependency | Declared | Used by | Rust | Fit | Note |
| --- | --- | --- | --- | --- | --- |
| `k8s.io/klog/v2` | yes | 11 | ? | unknown |  |
| `k8s.io/client-go/tools/cache` | no | 10 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/apis/meta/v1` | no | 6 | ? | unknown |  |
| `k8s.io/client-go/kubernetes` | no | 6 | ? | unknown |  |
| `k8s.io/client-go/rest` | no | 5 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/api/meta` | no | 4 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/runtime` | no | 4 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/watch` | no | 4 | ? | unknown |  |
| `github.com/prometheus/common/expfmt` | no | 3 | ? | unknown |  |
| `github.com/prometheus/common/version` | no | 3 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/apis/meta/v1/unstructured` | no | 3 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/runtime/schema` | no | 3 | ? | unknown |  |
| `github.com/prometheus/common/model` | no | 2 | ? | unknown |  |
| `k8s.io/api/apps/v1` | no | 2 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/api/resource` | no | 2 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/fields` | no | 2 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/util/validation` | no | 2 | ? | unknown |  |
| `k8s.io/client-go/dynamic` | no | 2 | ? | unknown |  |
| `k8s.io/client-go/tools/clientcmd` | no | 2 | ? | unknown |  |
| `k8s.io/component-base/metrics` | no | 2 | ? | unknown |  |
| `github.com/KimMachineGun/automemlimit/memlimit` | no | 1 | ? | unknown |  |
| `github.com/dgryski/go-jump` | yes | 1 | ? | unknown |  |
| `github.com/fsnotify/fsnotify` | yes | 1 | ? | unknown |  |
| `github.com/go-logr/logr` | yes | 1 | ? | unknown |  |
| `github.com/gobuffalo/flect` | yes | 1 | ? | unknown |  |
| `github.com/google/go-cmp/cmp` | no | 1 | ? | unknown |  |
| `github.com/netresearch/go-cron` | yes | 1 | ? | unknown |  |
| `github.com/oklog/run` | yes | 1 | ? | unknown |  |
| `github.com/prometheus/client_model/go` | no | 1 | ? | unknown |  |
| `github.com/prometheus/exporter-toolkit/web` | no | 1 | ? | unknown |  |
| `k8s.io/api/admissionregistration/v1` | no | 1 | ? | unknown |  |
| `k8s.io/api/autoscaling/v2` | no | 1 | ? | unknown |  |
| `k8s.io/api/batch/v1` | no | 1 | ? | unknown |  |
| `k8s.io/api/certificates/v1` | no | 1 | ? | unknown |  |
| `k8s.io/api/coordination/v1` | no | 1 | ? | unknown |  |
| `k8s.io/api/core/v1` | no | 1 | ? | unknown |  |
| `k8s.io/api/discovery/v1` | no | 1 | ? | unknown |  |
| `k8s.io/api/networking/v1` | no | 1 | ? | unknown |  |
| `k8s.io/api/policy/v1` | no | 1 | ? | unknown |  |
| `k8s.io/api/rbac/v1` | no | 1 | ? | unknown |  |
| `k8s.io/api/storage/v1` | no | 1 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/api/errors` | no | 1 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/types` | no | 1 | ? | unknown |  |
| `k8s.io/apimachinery/pkg/util/intstr` | no | 1 | ? | unknown |  |
| `k8s.io/client-go/discovery` | no | 1 | ? | unknown |  |
| `k8s.io/client-go/dynamic/dynamicinformer` | no | 1 | ? | unknown |  |
| `k8s.io/client-go/plugin/pkg/client/auth` | no | 1 | ? | unknown |  |
| `k8s.io/client-go/util/watchlist` | no | 1 | ? | unknown |  |
| `k8s.io/component-helpers/storage/ephemeral` | no | 1 | ? | unknown |  |
| `k8s.io/sample-controller/pkg/apis/samplecontroller/v1alpha1` | no | 1 | ? | unknown |  |
| `sigs.k8s.io/controller-runtime/pkg/metrics/filters` | no | 1 | ? | unknown |  |
| `github.com/KimMachineGun/automemlimit` | yes | 0 | ? | unknown |  |
| `github.com/google/go-cmp` | yes | 0 | ? | unknown |  |
| `github.com/prometheus/client_model` | yes | 0 | ? | unknown |  |
| `github.com/prometheus/common` | yes | 0 | ? | unknown |  |
| `github.com/prometheus/exporter-toolkit` | yes | 0 | ? | unknown |  |
| `github.com/robfig/cron/v3` | yes | 0 | ? | unknown |  |
| `k8s.io/api` | yes | 0 | ? | unknown |  |
| `k8s.io/apimachinery` | yes | 0 | ? | unknown |  |
| `k8s.io/client-go` | yes | 0 | ? | unknown |  |
| `k8s.io/component-base` | yes | 0 | ? | unknown |  |
| `k8s.io/component-helpers` | yes | 0 | ? | unknown |  |
| `k8s.io/sample-controller` | yes | 0 | ? | unknown |  |
| `k8s.io/utils` | yes | 0 | ? | unknown |  |
| `sigs.k8s.io/controller-runtime` | yes | 0 | ? | unknown |  |
| `github.com/prometheus/client_golang` (also `github.com/prometheus/client_golang/prometheus`, `github.com/prometheus/client_golang/prometheus/collectors`, `github.com/prometheus/client_golang/prometheus/collectors/version`, `github.com/prometheus/client_golang/prometheus/promauto`, `github.com/prometheus/client_golang/prometheus/promhttp`) | yes | 8 | prometheus | good |  |
| `sigs.k8s.io/yaml` (also `sigs.k8s.io/yaml/goyaml.v3`) | yes | 3 | serde_yaml_ng | good |  |
| `github.com/spf13/cobra` | yes | 2 | clap | good |  |
| `github.com/dlclark/regexp2/v2` | yes | 1 | fancy-regex | good |  |
| `github.com/stretchr/testify` | yes | 0 | std test, pretty_assertions | good |  |

### Pieces, in port order

- wave 0: `pkg/constant/` (17 lines)
- wave 0: `pkg/metric/` (157 lines)
- wave 0: `pkg/options/` (616 lines)
- wave 0: `pkg/sharding/` (200 lines)
- wave 0: `pkg/util/proc/` (60 lines)
- wave 0: `pkg/watch/` (134 lines)
- wave 0: `tests/e2e/framework/` (152 lines)
- wave 0: `tests/lib/` (14 lines)
- wave 1: `pkg/metric_generator/` (125 lines)
- wave 1: `pkg/metrics_store/` (418 lines)
- wave 2: `pkg/allowdenylist/` (131 lines)
- wave 2: `pkg/customresource/` (23 lines)
- wave 2: `pkg/optin/` (53 lines)
- wave 3: `pkg/builder/types/` (60 lines)
- wave 3: `pkg/util/` (116 lines)
- wave 4: `internal/store/` (11410 lines)
- wave 4: `pkg/metricshandler/` (295 lines)
- wave 5: `internal/discovery/` (600 lines)
- wave 5: `pkg/builder/` (98 lines)
- wave 6: `pkg/customresourcestate/` (975 lines)
- wave 7: `pkg/app/` (601 lines)
- wave 8: `internal/` (234 lines)
- wave 9: `main.go` (30 lines)

### Decisions for the user

**D1** (shape): The app is a cli, entered through main.go. Port every entry point, in that shape?

> Yes: a clap front door (`app front-door`) takes every command over first

**D2** (dependency k8s.io/klog/v2): Which crate replaces k8s.io/klog/v2 (used by 11 units)?

> Research it against primary sources into rust/research/k8s.io-klog-v2.md before porting what uses it

**D3** (dependency k8s.io/client-go/tools/cache): Which crate replaces k8s.io/client-go/tools/cache (used by 10 units)?

> Research it against primary sources into rust/research/k8s.io-client-go-tools-cache.md before porting what uses it

**D4** (dependency k8s.io/apimachinery/pkg/apis/meta/v1): Which crate replaces k8s.io/apimachinery/pkg/apis/meta/v1 (used by 6 units)?

> Research it against primary sources into rust/research/k8s.io-apimachinery-pkg-apis-meta-v1.md before porting what uses it

**D5** (dependency k8s.io/client-go/kubernetes): Which crate replaces k8s.io/client-go/kubernetes (used by 6 units)?

> Research it against primary sources into rust/research/k8s.io-client-go-kubernetes.md before porting what uses it

**D6** (dependency k8s.io/client-go/rest): Which crate replaces k8s.io/client-go/rest (used by 5 units)?

> Research it against primary sources into rust/research/k8s.io-client-go-rest.md before porting what uses it

**D7** (dependency k8s.io/apimachinery/pkg/api/meta): Which crate replaces k8s.io/apimachinery/pkg/api/meta (used by 4 units)?

> Research it against primary sources into rust/research/k8s.io-apimachinery-pkg-api-meta.md before porting what uses it

**D8** (dependency k8s.io/apimachinery/pkg/runtime): Which crate replaces k8s.io/apimachinery/pkg/runtime (used by 4 units)?

> Research it against primary sources into rust/research/k8s.io-apimachinery-pkg-runtime.md before porting what uses it

**D9** (dependency k8s.io/apimachinery/pkg/watch): Which crate replaces k8s.io/apimachinery/pkg/watch (used by 4 units)?

> Research it against primary sources into rust/research/k8s.io-apimachinery-pkg-watch.md before porting what uses it

**D10** (dependency github.com/prometheus/common/expfmt): Which crate replaces github.com/prometheus/common/expfmt (used by 3 units)?

> Research it against primary sources into rust/research/github.com-prometheus-common-expfmt.md before porting what uses it

**D11** (dependency github.com/prometheus/common/version): Which crate replaces github.com/prometheus/common/version (used by 3 units)?

> Research it against primary sources into rust/research/github.com-prometheus-common-version.md before porting what uses it

**D12** (dependency k8s.io/apimachinery/pkg/apis/meta/v1/unstructured): Which crate replaces k8s.io/apimachinery/pkg/apis/meta/v1/unstructured (used by 3 units)?

> Research it against primary sources into rust/research/k8s.io-apimachinery-pkg-apis-meta-v1-unstructured.md before porting what uses it

**D13** (dependency k8s.io/apimachinery/pkg/runtime/schema): Which crate replaces k8s.io/apimachinery/pkg/runtime/schema (used by 3 units)?

> Research it against primary sources into rust/research/k8s.io-apimachinery-pkg-runtime-schema.md before porting what uses it

**D14** (dependency github.com/prometheus/common/model): Which crate replaces github.com/prometheus/common/model (used by 2 units)?

> Research it against primary sources into rust/research/github.com-prometheus-common-model.md before porting what uses it

**D15** (dependency k8s.io/api/apps/v1): Which crate replaces k8s.io/api/apps/v1 (used by 2 units)?

> Research it against primary sources into rust/research/k8s.io-api-apps-v1.md before porting what uses it

**D16** (dependency k8s.io/apimachinery/pkg/api/resource): Which crate replaces k8s.io/apimachinery/pkg/api/resource (used by 2 units)?

> Research it against primary sources into rust/research/k8s.io-apimachinery-pkg-api-resource.md before porting what uses it

**D17** (dependency k8s.io/apimachinery/pkg/fields): Which crate replaces k8s.io/apimachinery/pkg/fields (used by 2 units)?

> Research it against primary sources into rust/research/k8s.io-apimachinery-pkg-fields.md before porting what uses it

**D18** (dependency k8s.io/apimachinery/pkg/util/validation): Which crate replaces k8s.io/apimachinery/pkg/util/validation (used by 2 units)?

> Research it against primary sources into rust/research/k8s.io-apimachinery-pkg-util-validation.md before porting what uses it

**D19** (dependency k8s.io/client-go/dynamic): Which crate replaces k8s.io/client-go/dynamic (used by 2 units)?

> Research it against primary sources into rust/research/k8s.io-client-go-dynamic.md before porting what uses it

**D20** (dependency k8s.io/client-go/tools/clientcmd): Which crate replaces k8s.io/client-go/tools/clientcmd (used by 2 units)?

> Research it against primary sources into rust/research/k8s.io-client-go-tools-clientcmd.md before porting what uses it

**D21** (dependency k8s.io/component-base/metrics): Which crate replaces k8s.io/component-base/metrics (used by 2 units)?

> Research it against primary sources into rust/research/k8s.io-component-base-metrics.md before porting what uses it

**D22** (dependency github.com/KimMachineGun/automemlimit/memlimit): Which crate replaces github.com/KimMachineGun/automemlimit/memlimit (used by 1 unit)?

> Research it against primary sources into rust/research/github.com-KimMachineGun-automemlimit-memlimit.md before porting what uses it

**D23** (dependency github.com/dgryski/go-jump): Which crate replaces github.com/dgryski/go-jump (used by 1 unit)?

> Research it against primary sources into rust/research/github.com-dgryski-go-jump.md before porting what uses it

**D24** (dependency github.com/fsnotify/fsnotify): Which crate replaces github.com/fsnotify/fsnotify (used by 1 unit)?

> Research it against primary sources into rust/research/github.com-fsnotify-fsnotify.md before porting what uses it

**D25** (dependency github.com/go-logr/logr): Which crate replaces github.com/go-logr/logr (used by 1 unit)?

> Research it against primary sources into rust/research/github.com-go-logr-logr.md before porting what uses it

**D26** (dependency github.com/gobuffalo/flect): Which crate replaces github.com/gobuffalo/flect (used by 1 unit)?

> Research it against primary sources into rust/research/github.com-gobuffalo-flect.md before porting what uses it

**D27** (dependency github.com/google/go-cmp/cmp): Which crate replaces github.com/google/go-cmp/cmp (used by 1 unit)?

> Research it against primary sources into rust/research/github.com-google-go-cmp-cmp.md before porting what uses it

**D28** (dependency github.com/netresearch/go-cron): Which crate replaces github.com/netresearch/go-cron (used by 1 unit)?

> Research it against primary sources into rust/research/github.com-netresearch-go-cron.md before porting what uses it

**D29** (dependency github.com/oklog/run): Which crate replaces github.com/oklog/run (used by 1 unit)?

> Research it against primary sources into rust/research/github.com-oklog-run.md before porting what uses it

**D30** (dependency github.com/prometheus/client_model/go): Which crate replaces github.com/prometheus/client_model/go (used by 1 unit)?

> Research it against primary sources into rust/research/github.com-prometheus-client_model-go.md before porting what uses it

**D31** (dependency github.com/prometheus/exporter-toolkit/web): Which crate replaces github.com/prometheus/exporter-toolkit/web (used by 1 unit)?

> Research it against primary sources into rust/research/github.com-prometheus-exporter-toolkit-web.md before porting what uses it

**D32** (dependency k8s.io/api/admissionregistration/v1): Which crate replaces k8s.io/api/admissionregistration/v1 (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-api-admissionregistration-v1.md before porting what uses it

**D33** (dependency k8s.io/api/autoscaling/v2): Which crate replaces k8s.io/api/autoscaling/v2 (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-api-autoscaling-v2.md before porting what uses it

**D34** (dependency k8s.io/api/batch/v1): Which crate replaces k8s.io/api/batch/v1 (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-api-batch-v1.md before porting what uses it

**D35** (dependency k8s.io/api/certificates/v1): Which crate replaces k8s.io/api/certificates/v1 (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-api-certificates-v1.md before porting what uses it

**D36** (dependency k8s.io/api/coordination/v1): Which crate replaces k8s.io/api/coordination/v1 (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-api-coordination-v1.md before porting what uses it

**D37** (dependency k8s.io/api/core/v1): Which crate replaces k8s.io/api/core/v1 (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-api-core-v1.md before porting what uses it

**D38** (dependency k8s.io/api/discovery/v1): Which crate replaces k8s.io/api/discovery/v1 (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-api-discovery-v1.md before porting what uses it

**D39** (dependency k8s.io/api/networking/v1): Which crate replaces k8s.io/api/networking/v1 (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-api-networking-v1.md before porting what uses it

**D40** (dependency k8s.io/api/policy/v1): Which crate replaces k8s.io/api/policy/v1 (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-api-policy-v1.md before porting what uses it

**D41** (dependency k8s.io/api/rbac/v1): Which crate replaces k8s.io/api/rbac/v1 (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-api-rbac-v1.md before porting what uses it

**D42** (dependency k8s.io/api/storage/v1): Which crate replaces k8s.io/api/storage/v1 (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-api-storage-v1.md before porting what uses it

**D43** (dependency k8s.io/apimachinery/pkg/api/errors): Which crate replaces k8s.io/apimachinery/pkg/api/errors (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-apimachinery-pkg-api-errors.md before porting what uses it

**D44** (dependency k8s.io/apimachinery/pkg/types): Which crate replaces k8s.io/apimachinery/pkg/types (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-apimachinery-pkg-types.md before porting what uses it

**D45** (dependency k8s.io/apimachinery/pkg/util/intstr): Which crate replaces k8s.io/apimachinery/pkg/util/intstr (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-apimachinery-pkg-util-intstr.md before porting what uses it

**D46** (dependency k8s.io/client-go/discovery): Which crate replaces k8s.io/client-go/discovery (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-client-go-discovery.md before porting what uses it

**D47** (dependency k8s.io/client-go/dynamic/dynamicinformer): Which crate replaces k8s.io/client-go/dynamic/dynamicinformer (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-client-go-dynamic-dynamicinformer.md before porting what uses it

**D48** (dependency k8s.io/client-go/plugin/pkg/client/auth): Which crate replaces k8s.io/client-go/plugin/pkg/client/auth (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-client-go-plugin-pkg-client-auth.md before porting what uses it

**D49** (dependency k8s.io/client-go/util/watchlist): Which crate replaces k8s.io/client-go/util/watchlist (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-client-go-util-watchlist.md before porting what uses it

**D50** (dependency k8s.io/component-helpers/storage/ephemeral): Which crate replaces k8s.io/component-helpers/storage/ephemeral (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-component-helpers-storage-ephemeral.md before porting what uses it

**D51** (dependency k8s.io/sample-controller/pkg/apis/samplecontroller/v1alpha1): Which crate replaces k8s.io/sample-controller/pkg/apis/samplecontroller/v1alpha1 (used by 1 unit)?

> Research it against primary sources into rust/research/k8s.io-sample-controller-pkg-apis-samplecontroller-v1alpha1.md before porting what uses it

**D52** (dependency sigs.k8s.io/controller-runtime/pkg/metrics/filters): Which crate replaces sigs.k8s.io/controller-runtime/pkg/metrics/filters (used by 1 unit)?

> Research it against primary sources into rust/research/sigs.k8s.io-controller-runtime-pkg-metrics-filters.md before porting what uses it

**D53** (dependency github.com/KimMachineGun/automemlimit): Which crate replaces github.com/KimMachineGun/automemlimit (declared, no unit imports it)?

> Research it against primary sources into rust/research/github.com-KimMachineGun-automemlimit.md before porting what uses it

**D54** (dependency github.com/google/go-cmp): Which crate replaces github.com/google/go-cmp (declared, no unit imports it)?

> Research it against primary sources into rust/research/github.com-google-go-cmp.md before porting what uses it

**D55** (dependency github.com/prometheus/client_model): Which crate replaces github.com/prometheus/client_model (declared, no unit imports it)?

> Research it against primary sources into rust/research/github.com-prometheus-client_model.md before porting what uses it

**D56** (dependency github.com/prometheus/common): Which crate replaces github.com/prometheus/common (declared, no unit imports it)?

> Research it against primary sources into rust/research/github.com-prometheus-common.md before porting what uses it

**D57** (dependency github.com/prometheus/exporter-toolkit): Which crate replaces github.com/prometheus/exporter-toolkit (declared, no unit imports it)?

> Research it against primary sources into rust/research/github.com-prometheus-exporter-toolkit.md before porting what uses it

**D58** (dependency github.com/robfig/cron/v3): Which crate replaces github.com/robfig/cron/v3 (declared, no unit imports it)?

> Research it against primary sources into rust/research/github.com-robfig-cron-v3.md before porting what uses it

**D59** (dependency k8s.io/api): Which crate replaces k8s.io/api (declared, no unit imports it)?

> Research it against primary sources into rust/research/k8s.io-api.md before porting what uses it

**D60** (dependency k8s.io/apimachinery): Which crate replaces k8s.io/apimachinery (declared, no unit imports it)?

> Research it against primary sources into rust/research/k8s.io-apimachinery.md before porting what uses it

**D61** (dependency k8s.io/client-go): Which crate replaces k8s.io/client-go (declared, no unit imports it)?

> Research it against primary sources into rust/research/k8s.io-client-go.md before porting what uses it

**D62** (dependency k8s.io/component-base): Which crate replaces k8s.io/component-base (declared, no unit imports it)?

> Research it against primary sources into rust/research/k8s.io-component-base.md before porting what uses it

**D63** (dependency k8s.io/component-helpers): Which crate replaces k8s.io/component-helpers (declared, no unit imports it)?

> Research it against primary sources into rust/research/k8s.io-component-helpers.md before porting what uses it

**D64** (dependency k8s.io/sample-controller): Which crate replaces k8s.io/sample-controller (declared, no unit imports it)?

> Research it against primary sources into rust/research/k8s.io-sample-controller.md before porting what uses it

**D65** (dependency k8s.io/utils): Which crate replaces k8s.io/utils (declared, no unit imports it)?

> Research it against primary sources into rust/research/k8s.io-utils.md before porting what uses it

**D66** (dependency sigs.k8s.io/controller-runtime): Which crate replaces sigs.k8s.io/controller-runtime (declared, no unit imports it)?

> Research it against primary sources into rust/research/sigs.k8s.io-controller-runtime.md before porting what uses it

