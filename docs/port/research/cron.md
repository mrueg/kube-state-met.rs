# CronJob schedules: netresearch/go-cron vs Rust crates

Versions checked: `github.com/netresearch/go-cron v0.19.0` (go.mod:13), `croner 5.0.1`, `cron 0.17.0`,
`chrono 0.4.45`, `chrono-tz 0.10.4`. `$GC` = `~/go/pkg/mod/github.com/netresearch/go-cron@v0.19.0`,
`$CR` = `~/.cargo/registry/src/index.crates.io-*`. All "Go:" results come from running the exact KSM parser
in the scratchpad (`gocheck/cronv/main.go`, `cron_vectors*.json`, `TZ=UTC`). The Rust comparisons come from
`regex-research/src/bin/cr.rs`.

## 1. What KSM computes (`internal/store/cronjob.go`)

- Parser: `cron.MustNewParser(Minute|Hour|Dom|Month|Dow|Descriptor|RobfigCompat)` (`:382`). There are exactly 5 fields,
  no seconds and no year. The comment says the intent is to match the k8s CronJob controller, which uses robfig/cron/v3 (`:379-381`).
- `parseSchedule` (`:384-394`): if `spec.timeZone != nil`, the schedule string becomes `"CRON_TZ=<tz> <schedule>"`.
- `getNextScheduledTime` (`:396-408`) returns `sched.Next(status.lastScheduleTime)` if it is set. Otherwise it returns
  `sched.Next(metadata.creationTimestamp)`. If both are zero it returns an error. **Wall-clock "now" is never used.**
  `(*metav1.Time).IsZero()` is nil-safe (`apimachinery@v0.37.2/pkg/apis/meta/v1/time.go:61-66`).
- `kube_cronjob_next_schedule_time` (`:244-269`) emits `float64(next.Unix())` unless the parse fails or
  `spec.suspend == true`. `kube_cronjob_schedule_invalid` (`:271-292`) emits 1 when the parse fails.
  `kube_cronjob_info{timezone}` uses `"local"` when `spec.timeZone` is nil (`:104-107`).
- Input times come from `metav1.Time`, which is RFC 3339 with whole seconds, converted with `.Local()` (`time.go:117`). The
  `time/tzdata` import (`cronjob.go:28`) embeds the IANA database, so zone lookup does not depend on the host.

## 2. go-cron semantics with these flags

`RobfigCompat = DowOrDom|LenientSteps|EveryRoundUp|DSTSkip|DowStepToSaturday` (`$GC/parser.go:84`).
`StepWildcard` is **not** set.

| Aspect | Behaviour (source) |
|---|---|
| Spec length | empty → error. More than 1024 bytes → error (`parser.go:391,491-496`) |
| TZ prefix | `TZ=` or `CRON_TZ=` only as a **prefix**. The name ends at the first space. A matching pair of `"`/`'` quotes is stripped. The name must be 1-64 chars of `[A-Za-z0-9/_+:-]`. Then `time.LoadLocation` (`parser.go:402-441,1630-1644`). No prefix means `time.Local` |
| Descriptors | `@yearly @annually @monthly @weekly @daily @midnight @hourly` (exact, **case-sensitive**), `@every <dur>`, `@every_aligned`, `@every_start`, `@triggered/@manual/@none` (`parser.go:1737-1797`). A descriptor is recognised only if the spec (after the TZ prefix) **starts** with `@` (`parser.go:505`) |
| Fields | `strings.Fields` (any whitespace). Exactly 5 fields (`parser.go:753-770`) |
| Bounds | min 0-59, hour 0-23, dom 1-31, month 1-12, dow **0-7** (7 = Sunday). Names are case-insensitive, 3-letter `jan..dec`, `sun..sat` (`spec.go:100-128,1599-1606`) |
| Range syntax | `*`, `?` (same as `*`), `a`, `a-b`, `a-b/s`, `a/s` (= `a-max/s`), `*/s`, lists via `,` (empty items ignored). Numbers are parsed with `strconv.Atoi`, so `+5` is accepted (`parser.go:1290-1420,1646-1656`) |
| Wraparound | `22-2`, `FRI-MON`, `NOV-FEB`, `30-2` (dom) are all accepted (`spec.go:100-118`) |
| Step | step 0 → error. With `LenientSteps`, a step ≥ the range size is OK (`*/60` = minute 0) (`parser.go:1316-1334`) |
| Wildcard bit | `*` or `?` with step 1 sets `starBit`. **Any step greater than 1 clears it** (`parser.go:1390-1392`) |
| DOM/DOW | `DowOrDom`: if either field carries `starBit`, the days are ANDed, otherwise ORed (`spec.go:808-815`) |
| `DowStepToSaturday` | an `N/step` in dow stops at 6, so `1/2` = Mon, Wed, Fri (no Sun=7) (`parser.go:624-629,1381-1388`) |
| Extensions | `L`, `W`, `#`, `H` → **errors** (DomL/DomW/DowNth/Hash not set) (`parser.go:668-706`) |
| Next start | `t` is truncated to the second, plus 1 s, and the search runs in the schedule zone. If no zone was given, `t.Location()` is used (`spec.go:539-552`). The result is strictly after `t` |
| No match | search limit is 5 years (`spec.go:145`). After that `Next` returns **`time.Time{}`**, which KSM emits as **-62135596800** |
| DST gap | `DSTSkip`: wall times that do not exist are skipped, with no catch-up run (`spec.go:670-679`) |
| DST overlap | `Next` returns **both** instants of a repeated wall time (doc `parser.go:75-77`) |
| `@every d` | Go `time.ParseDuration`. `d < 1s` is rounded up to 1 s (`EveryRoundUp`) and sub-seconds are truncated. `Next(t) = t + d - t.nanos` (`constantdelay.go:47-88`) |
| `@triggered` | `Next` = zero time (`triggered.go:24`) → -62135596800 |

## 3. Test vectors (Go-verified, process `TZ=UTC`; `from` is lastScheduleTime/creationTimestamp)

```
schedule              tz                 from                    → next (UTC)              unix
*/5 * * * *           nil                2024-01-01T00:00:00Z    2024-01-01T00:05:00Z      1704067500
0 * * * *             nil                2024-01-01T00:59:59Z    2024-01-01T01:00:00Z
@weekly               nil                2024-01-01T00:30:00Z    2024-01-07T00:00:00Z      (Sunday)
@HOURLY               nil                -                       ERR unrecognized descriptor
@every 90s            nil                2024-01-01T00:00:30Z    2024-01-01T00:02:00Z
@every 1.5h           nil                2024-01-01T00:00:30Z    2024-01-01T01:30:30Z
@every 500ms | 0s | -1h nil              2024-01-01T00:00:30Z    2024-01-01T00:00:31Z      (rounded to 1s)
@every 1d             nil                -                       ERR unknown unit "d"
@triggered            nil                any                     0001-01-01T00:00:00Z      -62135596800
@reboot               nil                -                       ERR unrecognized descriptor
" @daily"             nil                -                       ERR expected exactly 5 fields, found 1
"@daily "             nil                -                       ERR unrecognized descriptor
"  0  *\t* * *  "     nil                2024-01-01T00:00:00Z    2024-01-01T01:00:00Z
0 0 13 * FRI          nil                2024-01-01T00:00:00Z    2024-01-05T00:00:00Z      (OR)
0 0 */2 * 1           nil                2024-01-01T12:00:00Z    2024-01-03T00:00:00Z      (OR: */2 is not a wildcard)
0 0 */1 * 1           nil                2024-01-02T00:00:00Z    2024-01-08T00:00:00Z      (AND: */1 is)
0 0 1-31 * 1          nil                2024-01-02T00:00:00Z    2024-01-03T00:00:00Z      (OR)
0 0 * * 7 | 0 | sun   nil                2024-01-01T00:00:00Z    2024-01-07T00:00:00Z
0 0 * * FRI-MON       nil                2024-01-02T00:00:00Z    2024-01-05T00:00:00Z
0 0 * * 1/2           nil                2024-01-06T00:00:00Z    2024-01-08T00:00:00Z      (Sun excluded)
0 0 * * 1-7/2         nil                2024-01-06T00:00:00Z    2024-01-07T00:00:00Z      (Sun included)
0 0 * * 5/2           nil                2024-01-01T00:00:00Z    2024-01-05T00:00:00Z
0 0 1 nov-feb *       nil                2024-03-01T00:00:00Z    2024-11-01T00:00:00Z
0 22-2 * * *          nil                2024-01-01T03:00:00Z    2024-01-01T22:00:00Z
0 0 30-2 * *          nil                2024-01-03T00:00:00Z    2024-01-30T00:00:00Z
*/60 * * * *          nil                2024-01-01T00:00:00Z    2024-01-01T01:00:00Z
0 */24 * * *          nil                2024-01-01T00:00:00Z    2024-01-02T00:00:00Z
0 0 ? * *             nil                2024-01-01T00:00:00Z    2024-01-02T00:00:00Z
0,,30 * * * *         nil                2024-01-01T00:10:00Z    2024-01-01T00:30:00Z
+5 * * * *            nil                2024-01-01T00:00:00Z    2024-01-01T00:05:00Z
0 0 30 2 *            nil                2024-01-01T00:00:00Z    0001-01-01T00:00:00Z      -62135596800
0 0 29 2 *            nil                2024-03-01T00:00:00Z    2028-02-29T00:00:00Z
0 0 L * * | 0 0 15W * * | 0 0 * * 5#3 | H * * * *              ERR (extension not enabled)
0 0 0 * * * (6 fields) | 0 0 * * (4) | "" | 60 * * * * | 0 24 * * * | 0 0 * * 8 | */0 * * * *  ERR
TZ=UTC 0 9 * * *      nil                2024-01-01T00:00:00Z    2024-01-01T09:00:00Z
TZ="Asia/Tokyo" 0 9 * * * nil            2024-01-01T00:00:00Z    2024-01-02T00:00:00Z
0 9 * * *             Asia/Tokyo         2024-01-01T00:00:00Z    2024-01-02T00:00:00Z
0 9 * * *             Asia/Kolkata       2024-01-01T00:00:00Z    2024-01-01T03:30:00Z
0 9 * * *             EST | Etc/GMT+5    2024-01-01T00:00:00Z    2024-01-01T14:00:00Z
0 9 * * *             Local              2024-01-01T00:00:00Z    2024-01-01T09:00:00Z      (= process TZ!)
0 9 * * *             "" | utc | Mars/Olympus                    ERR
TZ=UTC 0 9 * * *      Asia/Tokyo         -                       ERR expected exactly 5 fields, found 6
@hourly               Asia/Kolkata       2024-01-01T00:00:00Z    2024-01-01T00:30:00Z
30 2 * * *            America/New_York   2024-03-09T12:00:00Z    2024-03-11T06:30:00Z      (gap day skipped)
30 1 * * *            America/New_York   2024-11-03T05:00:00Z    2024-11-03T05:30:00Z      (1st pass, EDT)
30 1 * * *            America/New_York   2024-11-03T05:30:00Z    2024-11-03T06:30:00Z      (2nd pass, EST)
0 0 * * *             America/Santiago   2024-09-07T12:00:00Z    2024-09-09T03:00:00Z      (midnight gap skipped)
0 0 1 * *             Pacific/Apia       2011-12-29T00:00:00Z    2011-12-31T10:00:00Z
```
With process `TZ=America/New_York` and `tz=nil`: `@daily` from `2024-01-01T00:30:00Z` gives `2024-01-01T05:00:00Z`.
The result depends on the pod's timezone.

## 4. Rust crates measured against the Go vectors

**croner 5.0.1** (`CronParser::builder().seconds(Disallowed).year(Disallowed)`, `find_next_occurrence(&t, false)`):
24 + 12 + 1 differences across the three vector files. Highlights:
- No `@every`, `@triggered` or `@midnight`. Descriptors are case-insensitive (`@HOURLY` is accepted) and whitespace is trimmed
  (`$CR/croner-5.0.1/src/parser.rs:161-166,392-403`). No `TZ=`/`CRON_TZ=` prefix support.
- Rejects wraparound ranges (`22-2`, `FRI-MON`, `NOV-FEB`, `30-2`), `N/step` (`5/20`, `1/2`; `sloppy_ranges`,
  `parser.rs:525`, would be needed) and `+5`. **Accepts** `L`, `W` and `#` (Go rejects them).
- DOM/DOW: OR by default, and it agrees on `*/2` (OR). It **disagrees on `*/1`**: croner ORs, Go ANDs.
- DST: in a gap, croner runs a fixed-time job at the first valid time (`2024-03-10T07:00Z`; Go skips to the 11th).
  In an overlap it runs once (Go's `Next` returns the 2nd pass) (`README.md:488-510`, `parser.rs:71-90`).
  Santiago midnight gap: croner returns 09-08 04:00Z, Go returns 09-09.
- An impossible date returns an error (`time search limit exceeded`), where Go returns the zero time.

**cron 0.17.0**: requires a seconds field, so `"0 " + spec` has to be prefixed. 28 differences. Day-of-week 1 = **Sunday**
(`$CR/cron-0.17.0/src/time_unit/days_of_week.rs:9-10`), so `7` = Saturday and `0` is an error. DOM/DOW are always ANDed
(`0 0 13 * FRI` → 2024-09-13). No `@midnight`/`@annually`/`@every`. Steps must be smaller than the range (`*/60`
is an error). No wraparound ranges. Not viable.

## 5. Recommendation: port the go-cron subset (do not use a crate)

Neither crate matches without a wrapper. Such a wrapper would need TZ prefixes, `@every`/`@triggered`, descriptor
case rules, wraparound ranges, `N/step`, the `*/1` star rule, Sun=7 step clipping, DST gap/overlap semantics, the zero-time
sentinel, and rejection of `L/W/#`. That is more code than the parser itself.

Measured size of the needed Go subset (function bodies plus comments, via awk over `$GC/parser.go` and `spec.go`):
the parser path (parseTimezone 47, parse 128, normalizeFields 45, getField 21, parseRangeBounds 26,
validateRange* 44, getRange 62, getWraparoundBits 29, getBits 23, parseIntOrName/mustParseInt 30,
validateTimezone/isValidTimezoneChar 30, parseDescriptor/intervalSchedule/descriptorSpec 79) is about **585 lines**. The `Next`
path in spec.go (nextExisting, advance*, dayMatches, DST helpers) is about **450 lines**, and `constantdelay.go`
is 104 lines. For comparison, all of parser.go plus spec.go is 2927 lines.

Suggested Rust shape (estimated 350-450 lines):
- `parse(spec, tz: Option<&str>) -> Result<Schedule>`. Prefix `CRON_TZ=` exactly as KSM does, then port `parseTimezone`.
  Resolve zones with `chrono_tz::Tz::from_str`, plus special handling for `"Local"` → process local zone and "no prefix" → process local zone
  (Go also resolves `"UTC"`. `chrono-tz` resolves `UTC`, `EST`, `Etc/GMT+5`, rejects `utc`/`Local`).
  Embed tzdata: `chrono-tz` already compiles it in, which mirrors `time/tzdata`.
- Bitsets: `u64` per field, keeping `starBit = 1<<63` exactly. Port `getRange`/`getWraparoundBits` line by line,
  including `extra = 0` when `step > 1`, and the `stepEnd = 6` drop for dow.
- `@every`: port Go `time.ParseDuration` (units `ns us µs ms s m h`, fractions, sign), then clamp to ≥1 s and truncate.
- `next(t)`: iterate local **dates** from `date(t+1s) - 1 day` for at most `year+5`. For each date where `dayMatches`
  holds and the month matches, enumerate matching hour×minute in order, then map with
  `tz.from_local_datetime(naive)`: `None` → skip (DSTSkip), `Single(u)` → candidate,
  `Ambiguous(a, b)` → both. Return the smallest candidate `> t` (truncated to the second). If none: return the zero-time sentinel
  (`i64` -62135596800). This reproduces all DST vectors above, including the Santiago and Apia cases. Make it
  generic over `chrono::TimeZone`, so `chrono::Local` covers `tz=nil`.
- Golden tests: the vectors in section 3, plus a fuzz/differential job against the Go harness.

## 6. Semantic traps
1. **"now" is never used**: next = Next(lastScheduleTime or creationTimestamp), so it can lie in the past.
2. **Zero-time sentinel**: `@triggered`, impossible dates (`0 0 30 2 *`) and any search past 5 years produce `Unix()` =
   -62135596800, which is emitted as a metric value. Don't map it to "no metric".
3. **tz = nil uses the process local zone** (`time.Local`, from `$TZ` or `/etc/localtime`), and so does `timeZone: Local`.
   The info metric says `"local"` in that case.
4. **DOM/DOW OR rule depends on the literal `*`**: `*/1` and `*` are wildcards, `*/2` and `1-31` are not.
5. **Sunday = 7 is accepted**, but `1/2` (any `N/step` in dow) stops at Saturday, while `1-7/2` and `*/s` do not.
6. **DST**: spring-forward times are *skipped* (the next day's run is used), and fall-back times fire *twice*. croner does the
   opposite in both cases.
7. Descriptors are case-sensitive and must be the whole remaining spec with no trailing space. Leading
   whitespace before `@` breaks detection, while leading whitespace before 5-field specs is fine.
8. `CRON_TZ=X` is prefixed by KSM, so a schedule that itself starts with `TZ=`/`CRON_TZ=` while `spec.timeZone`
   is set fails to parse (`6 fields`). That is reported via `kube_cronjob_schedule_invalid`.
9. Extensions `L W # H` and 6-field (seconds) specs are **invalid**, so `schedule_invalid` = 1, even though many crates accept them.
10. go-cron accepts specs that robfig/cron/v3 rejects (`7`, wraparound, quoted TZ, `@triggered`: `$GC/parser.go:66-70`).
    Mirror go-cron, not robfig. Whether the apiserver lets such schedules through was not checked here.
11. `@every` counts from lastScheduleTime and keeps its seconds (`00:00:30` + 90s = `00:02:00`). It is not aligned to the minute.
