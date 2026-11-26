# Kind → plural: gobuffalo/flect vs Rust inflection crates

Versions checked: `github.com/gobuffalo/flect v1.0.3` (go.mod:11), `pluralizer 0.6.0`, `Inflector 0.11.4`,
`cruet 1.0.0`. `$FL` = `~/go/pkg/mod/github.com/gobuffalo/flect@v1.0.3`. All outputs below were produced by
running `flect.Pluralize` (scratchpad `gocheck/flectv/main.go`, run with cwd `/`, input list `kinds.txt`,
results in `flect_gold.tsv`) and the Rust crates (`regex-research/src/bin/pl.rs`).

## 1. Where KSM uses it, and how much it matters

- `Resource.GetResourceName()` returns `ResourcePlural` if that is non-empty. Otherwise it returns
  `strings.ToLower(flect.Pluralize(Kind))` (`pkg/customresourcestate/config.go:91-98`). The result becomes
  `customResourceMetrics.ResourceName` (`custom_resource_metrics.go:58`). That value is the store `Name()` and
  the GVR `Resource` (`:62-75`).
- In the production path `FromConfig`, every configured resource is resolved through CRD discovery, and
  **`resource.ResourcePlural = resolved.Plural` is set unconditionally** (`config.go:197-202`). The plural comes from
  CRD `spec.names.plural`. A CRD without a plural is logged and skipped (`internal/discovery/discovery.go:91-93`).
  So flect is effectively a **fallback**. It is reached only when `ResourcePlural` is empty at
  `NewCustomResourceMetrics` time (unit tests, or future callers). A user-supplied `resourcePlural` is also
  overwritten by discovery in that path.
- Even so, port it exactly. It is cheap, and it keeps `GetResourceName` unit-test parity.

## 2. flect's algorithm (`$FL/pluralize.go:32-72`)

```
Pluralize(s) = New(s).Pluralize().String()
1. parts = toParts(s); last = parts[-1]                    (ident.go:31-95,101-106)
   if last == "": return ""
2. if singleToPlural[Original]   → ReplaceSuffix(Original, p)    (exact, case-sensitive, whole string)
3. if pluralToSingle[Original]   → unchanged
4. ls = lower(last); if pluralToSingle[ls] → unchanged
5. if singleToPlural[ls] = p     → p = Capitalize(p) if last == Capitalize(last); ReplaceSuffix(last, p)
6. for r in pluralRules: if HasSuffix(last, r.suffix)   ← CASE-SENSITIVE on `last`
                           → ReplaceSuffix(last, r.fn(last))
7. if HasSuffix(ls, "s") → unchanged
8. return Original + "s"
ReplaceSuffix(orig,new) = New(TrimSuffix(Original, orig) + new)   (ident.go:109-111)
```

**toParts** (`ident.go:31-95`) trims the input. If the whole uppercased string is an acronym, it is a single part.
Otherwise a new part starts at a space or one of `_ : - /` (`flect.go:11`), at an uppercase letter that follows a
non-uppercase letter, or at an uppercase letter when the current buffer is a known acronym (`HTTPRoute` → `HTTP`, `Route`). Letters,
digits, punctuation and backtick are kept. Any other rune ends the part. Parts that are acronyms are uppercased (`flect.go:22-34`).
The 144 acronyms are listed in `acronyms.go` (e.g. `HTTP HTTPS API DNS ID IP SQL UDP URL`). `TCP`, `GRPC` and `OCI` are *not* in the list, but
`TCPRoute`/`GRPCRoute`/`APIService` still come out as `TCPRoutes`/`GRPCRoutes`/`APIServices` (verified).

**Tables** (`plural_rules.go`):
- `dictionary` (`:42-208`, 126 words): irregulars (`child→children`, `person→people`, `man→men`,
  `tooth→teeth`, `index→indices`, `matrix` via rule, `quota→quotas`, `alias→aliases`, `base→bases`,
  `media→media`, `medium→media`), uncountables (`equipment information jeans money news rice`), and
  identicals (`sheep fish series species deer police you`…). `ID→IDs` is exact-case.
  - `singleToPlural[singular] = plural`. `pluralToSingle[plural] = singular` (and the alternative) unless
    `unidirectional` (`:224-254`).
- `singularToPluralSuffixList` (`:266-389`, 77 suffix pairs, in priority order): `tive→tives`,
  `eaf→eaves`, `oaf→oaves`, `afe→aves`, `arf→arves`, `rfe→rves`, `rf→rves`, `lf→lves`, `fe→ves`, `ay/ey/oy/uy→+s`,
  `quy→quies`, `y→ies`, `eau→eaux`, `[bdlnv]ula→bulae` (sic: dula, lula, nula and vula also map to "bulae"),
  `hedron→hedra`, `ium→ia`, `seum→seums`, `eum→ea`, `oum→oa`, about 25 more `-um→-a` variants (`imum→ima`, `trum→tra`, `atum→ata`…), `trix→trices`,
  `iasis mesis kinesis resis gnosis opsis ysis → -es`, `ouse→ouses`, `lause→lauses`, `us→uses`, `ch→ches`,
  `io→ios`, `sh→shes`, `ss→sses`, `ez→ezzes`, `iz→izzes`, `tz→tzes`, `zz→zzes`, `ano→anos`, `lo→los`, `to→tos`,
  `oo→oos`, `o→oes`, `x→xes`, `S→Ses` (uppercase, for acronyms).
- **Rule list build order** (`InsertPluralRule` *prepends* a pair `[noop(plural), rule(singular→plural)]`,
  `:15-28`). First the suffix list is inserted in reverse (`:392-395`), which leaves it in forward order. Then every non-`exact`
  dictionary word is prepended (`:398-417`). Final order: **dictionary words in reverse declaration order, then
  the suffix list in order**. Each entry first checks "already ends with the plural → unchanged". Evidence:
  `Human` → `Humans` (`human` is declared after `man`, so it wins), `Database` → `Databases` (dictionary `base`).
- **Custom data**: `init()` loads `inflections.json` / `acronyms.json` from the **current working directory**, or
  from `$INFLECT_PATH` / `$ACRONYMS_PATH` (`custom_data.go:14-43,65-88`). The Go KSM binary therefore changes its
  behaviour based on those env vars and files.

## 3. Test vectors: flect.Pluralize (then KSM lowercases)

| Kind | flect | lower | | Kind | flect | lower |
|---|---|---|---|---|---|---|
| Foo | Foos | foos | | Prometheus | Prometheuses | prometheuses |
| Ingress | Ingresses | ingresses | | Alertmanager | Alertmanagers | alertmanagers |
| Policy | Policies | policies | | ServiceMonitor | ServiceMonitors | servicemonitors |
| NetworkPolicy | NetworkPolicies | networkpolicies | | Database | Databases | databases |
| Status | Statuses | statuses | | Analysis | Analyses | analyses |
| Proxy | Proxies | proxies | | AnalysisRun | AnalysisRuns | analysisruns |
| **Index** | **Indices** | indices | | Redis / Postgres / Thanos | unchanged | redis / postgres / thanos |
| Matrix | Matrices | matrices | | Elasticsearch | Elasticsearches | elasticsearches |
| Quota / ResourceQuota | Quotas / ResourceQuotas | … | | MySQL | MySQLs | mysqls |
| Endpoints | Endpoints | endpoints | | **DNS** | **DNSes** | dnses |
| EndpointSlice | EndpointSlices | endpointslices | | ID | IDs | ids |
| Person | People | people | | DNSEndpoint | DNSEndpoints | dnsendpoints |
| Child | Children | children | | **Video** / **Repo** | **Videoes / Repoes** | videoes / repoes |
| Gateway / GatewayClass | Gateways / GatewayClasses | … | | Photo / Radio / Zoo / Echo | Photos / Radios / Zoos / Echoes | |
| HTTPRoute / GRPCRoute | HTTPRoutes / GRPCRoutes | httproutes | | **Safe** / **Cafe** | **Saves / Caves** | |
| VirtualService | VirtualServices | virtualservices | | Leaf / Life / Wolf / **Golf** | Leaves / Lives / Wolves / **Golves** | |
| Cluster / ClusterIssuer | Clusters / ClusterIssuers | | | Belief / Chief | Beliefs / Chiefs | |
| Certificate / Issuer | Certificates / Issuers | | | **Price** | **Price** (`rice` uncountable) | price |
| Registry / Canary | Registries / Canaries | | | Slice | Slices | slices |
| GitRepository / OCIRepository | GitRepositories / OCIRepositories | | | **Postman** | **Postmen** | postmen |
| EventBus | EventBuses | eventbuses | | Human | Humans | humans |
| Money / News / Information / Equipment | unchanged | | | **Bluetooth** | **Blueteeth** | |
| Series / Fish / Sheep | unchanged | | | Salesperson | Salespeople | |
| Media / Medium / Datum / Data | Media / Media / Data / Data | | | Warehouse / Mouse | Warehouses / Mice | |
| Premium / Minimum / Spectrum | Premia / Minima / Spectra | | | Forum / Checksum | Forums / Checksums | |
| Criterion | Criteria | | | Quiz / Waltz | Quizzes / Waltzes | |
| **Octopus** | **Octupuses** (typo in flect) | | | Radius / Corpus | Radii / Corpora | |
| Alias / Base / Case / Phase / Lease / Cache | Aliases / Bases / Cases / Phases / Leases / Caches | | | Schema / Metadata | Schemas / Metadata | |
| policy | policies | | | **POLICY** | **POLICYs** | policys |
| STATUS | STATUSes | statuses | | ingress | ingresses | |
| people / Children / Policies / Users | unchanged | | | SomeKind2 / Kind_v2 | SomeKind2s / Kind_v2s | |
| Ox / Die / Toe / Use / Axis / Louse | Oxen / Dice / Toes / Uses / Axes / Lice | | | Tooth / Goose | Teeth / Geese | |
| (empty) | (empty) | | | Box / Prefix / Vertex / Bus / Address | Boxes / Prefixes / Vertices / Buses / Addresses | |

Built-in kinds (Pod, Node, ConfigMap, Secret, Deployment, DaemonSet, StatefulSet, Job, CronJob,
HorizontalPodAutoscaler, PersistentVolumeClaim, StorageClass, VolumeAttachment, CustomResourceDefinition) and
common CRDs (Application, AppProject, Rollout, Kustomization, HelmRelease, HelmRepository, Bucket, Workflow,
Sensor, EventSource, PodMonitor, PrometheusRule) all just get `+s`/`+es`, as expected.

## 4. Rust crates against the 147 flect vectors (lowercased comparison)

| Crate | agrees | notable misses |
|---|---|---|
| `pluralizer 0.6.0` (`pluralize(k, 2, false)`) | 130/147 | Video→videos, Repo→repos, Safe/Cafe→safes/cafes, Money→monies, Price→prices, Bluetooth→bluetooths, Medium/Premium/Minimum/Spectrum→‑ums, Waltz→waltzs, Octopus→octopuses, Corpus→corpuses, Schema→schemata, POLICY→policies, DNS→dns |
| `Inflector 0.11.4` (`string::pluralize::to_plural`) | 98/147 | **EndpointSlice→endpointsliceice**, Slice→sliceice, Salesperson→salespersonople, Index→indexes, Person→persons, Child→childs, Fish/Sheep→fishes/sheeps, Alias→alias, Status→statuss, … |
| `cruet 1.0.0` (`string::pluralize::to_plural`) | 100/147 | **EndpointSlice→endpointslice** (unchanged), Index→indexes, Person→persons, Child→childs, Status→statuss, … |

None of the crates is a drop-in replacement. Inflector and cruet even mangle `EndpointSlice`. pluralizer is closest, but it
fixes flect's quirks (Videoes, Saves, Octupuses, POLICYs), so its output differs for real-world kinds
(`Video`, `Repo`), and it keeps global mutable rule state in a `Mutex`.

## 5. Recommendation: port flect's tables and algorithm verbatim

Size: `pluralize.go` 72 + `plural_rules.go` 417 (mostly data) + `ident.go` toParts/LastPart/ReplaceSuffix
~80 + `acronyms.go` 152 + `flect.go` helpers ~35 + `capitalize.go` ~10 ≈ 770 Go lines. Expect about 350 lines of Rust
(static arrays plus ~80 lines of logic). Implementation notes:
- Build `pluralRules` once (`LazyLock<Vec<(suffix, kind)>>`) in the **same order**: dictionary (non-exact)
  reversed, each as `[noop(plural), replace(singular→plural)]`, followed by the 77 suffix pairs in order. Generate
  the Rust tables from `plural_rules.go` with a script, and keep a golden test of all 147 vectors above.
- Use `char::is_uppercase` / `is_alphabetic` / `is_numeric` for Go's `unicode.IsUpper/IsLetter/IsDigit`.
  `unicode.IsPunct` is Unicode category P*: use `unicode-general-category`, or restrict to ASCII punctuation, because
  Kubernetes kinds are ASCII identifiers. Capitalize uses `unicode.ToTitle` on the first rune.
- Do **not** port the `inflections.json`/`INFLECT_PATH` loading unless parity with that env hook is wanted.
  Document the decision either way.
- Apply `to_lowercase()` after pluralizing, exactly like `config.go:97`.

## 6. Semantic traps
1. Flect is effectively dead in production: discovery overwrites `ResourcePlural` with the CRD's
   `spec.names.plural` (`config.go:201`), including any user-supplied `resourcePlural`.
2. Suffix rules match **case-sensitively** on the last CamelCase part, but dictionary lookups are lowercased.
   So `POLICY` → `POLICYs`, while `policy`/`Policy` → `policies`. Only the `S→Ses` rule is uppercase
   (`DNS` → `DNSes`, `STATUS` → `STATUSes`).
3. Only the **last** CamelCase part is inflected. `Postman` → `Postmen`, `Bluetooth` → `Blueteeth`, `Salesperson` →
   `Salespeople`, `ResourceQuota` → `ResourceQuotas`. Acronym splitting (`HTTPRoute` → `HTTP|Route`) relies on the
   acronym table. `GRPC`/`OCI` are not acronyms, but the result is the same here.
4. Anything already ending in lowercase `s` is returned unchanged (`Redis`, `Postgres`, `Thanos`, `Endpoints`). So are
   words in `pluralToSingle` (`Media`, `Data`, `People`, `Series`), and words that end with a rule's plural (`Policies`, `Buses`).
5. Real quirks to reproduce, not fix: `Index→Indices` (Kubernetes convention would be `indexes`), `Video→Videoes`,
   `Repo→Repoes`, `Safe→Saves`, `Cafe→Caves`, `Golf→Golves`, `Price→Price`, `Octopus→Octupuses`, `Premium→Premia`,
   `Medium→Media`.
6. The dictionary match on step 2 uses the whole `Original` string, case-sensitively (`ID` → `IDs`, but `Id` → `Ids` via the
   final `+s`). Step 5 re-capitalizes only if the last part was capitalized.
7. Go flect reads `inflections.json` from the cwd / `$INFLECT_PATH` at process start. A KSM container started in a
   directory with such a file pluralizes differently.
