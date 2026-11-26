# Regex engines: Go (regexp2 ECMAScript, RE2) vs Rust (fancy-regex 0.20, regex 1.x)

Versions checked: `github.com/dlclark/regexp2/v2 v2.13.1` (go.mod:8), Go 1.27.4 `regexp`,
`fancy-regex 0.20.2`, `regex 1.13.2` / `regex-syntax 0.8.12`.
Paths: `$GM` = `~/go/pkg/mod`, `$CR` = `~/.cargo/registry/src/index.crates.io-*`.
Every match/compile result below was produced by running both sides. The harnesses are in the scratchpad
(`gocheck/re2/main.go`, `regex-research/src/bin/re.rs`, `vectors.json`).

## 1. Call sites in kube-state-metrics

| Site | Engine | Pattern source | Haystack | Match call |
|---|---|---|---|---|
| `pkg/allowdenylist/allowdenylist.go:82` | regexp2, `ECMAScript` (`:35`) | `--metric-allowlist` / `--metric-denylist` entries | metric family name (`generator.Name`, `:168`) | `r.MatchString(item)` (`:125`), unanchored search |
| `pkg/optin/optin.go:65` | Go `regexp` (RE2) | `--metric-opt-in-list` entries | metric family name (`:38`) | `MatchString`, unanchored |
| `internal/store/utils.go:385` | Go `regexp` | `--metric-labels-allowlist` / annotations keys that contain `*` | k8s label/annotation **key** (`:320`) | `MatchString` on `^…$` built by `expandWildcard` (`:350-365`) |
| `pkg/customresourcestate/*` | **none** | – | – | `grep -rn 'regexp\|Regex' pkg/customresourcestate` gives no hits outside tests. CRS uses flect only (see flect.md). `internal/store/testutils.go:126` is test-only. |

Flag parsing: `MetricSet.Set` splits on `,` and trims whitespace (`pkg/options/types.go:54-63`), so a
pattern can never contain a comma. `a{1,3}` becomes the two entries `a{1` and `3}`.

The allow/deny and opt-in filters run once per family generator when the stores are built
(`pkg/metric_generator/filter.go:37,50`), not on every scrape.

## 2. regexp2 configuration actually in effect

- `regexp.DefaultMatchTimeout = time.Minute`, set once inside `New()` (`allowdenylist.go:49-51`). It is a
  package global (`$GM/github.com/dlclark/regexp2/v2@v2.13.1/regexp.go:25-26`) and is copied into each
  `Regexp.MatchTimeout` at compile time (`regexp.go:123`).
- The per-match backtracking stack is limited to 100 000 slots (`options.go:8`). When it is exceeded,
  the match returns `ErrBacktrackingStackLimit` (`regexp.go:31`).
- Any match error makes `IsIncluded` return `(false, err)` (`allowdenylist.go:126-128`). `Test()` logs it
  and returns **false** (`:169-171`). So the family is **dropped in both allowlist and denylist mode**.
- In v2, `ECMAScript` means "follow the ECMAScript standard" and not the C# behaviour
  (`$GM/.../regexp2/v2@v2.13.1/README.md:27,124`). `Unicode` (the `u` flag) is **not** set, so this is
  non-`/u` ECMAScript with Annex-B identity escapes.
- Measured: `(a+)+$` on `"a"*29+"b"` ran for the **full 60 s** and then returned `match timeout after 1m0s`.
  The stack limit did not trigger first. fancy-regex answers the same input in µs, because it hands
  the pattern to the `regex` DFA.

## 3. regexp2 ECMAScript vs fancy-regex 0.20 (default builder): verified differences

fancy-regex rows marked "=" behaved identically. 37 of 83 vectors differ.

| Pattern | Input | regexp2 ECMAScript | fancy-regex default |
|---|---|---|---|
| `kube_pod_.*`, `^kube_(?!pod_).*`, `(?<!kube_)pod`, `(?<=ab\|c)d`, `^(a)\1$`, `^(?<n>ku)_\k<n>$`, `(?'n'…)`, `(?i)`, `(?i:…)`, `(?m)`, `(?s)`, `(?x)`, `(?#c)`, `(?>a)`, `a*?` | various | match | = |
| `(?<=a+)b` (variable-length lookbehind) | `aab` / `b` | true / false | = (accepted in 0.20) |
| `^(?P<n>ku)$` | `ku` | **compile error** | true |
| `^(?<a>x)(?<a>y)$` | `xy` | **compile error** (dup name) | true |
| `^a++$` | `aa` | **compile error** | true (possessive) |
| `^\d$` | `٣`, `５` | false, false | **true, true** (Unicode) |
| `^\w$` | `é` | false | **true** |
| `^\s$` | U+FEFF / U+0085 | true / false | **false / true** |
| `\bpod\b` | `épodé` | true (ASCII `\w`) | **false** |
| `^.$` | `\r`, U+2028 | false, false | **true, true** |
| `^a\z` / `^\Aa` / `^a\Z` | `a` | **false** (`\z`=`z`, `\A`=`A`, `\Z`=`Z`) | true (anchors) |
| `^\h$` / `^\e$` / `^\a$` | `h` / `e` / `a` | true (identity) | false (hex-digit / ESC / BEL) |
| `^\K$` / `^\G$` / `^\N$` / `^\R$` | `K` / `G` / `a` / `R` | true / true / false / true | false / false / **true** / false |
| `^\X$` | `X` | true | compile error |
| `^\Qa.b\E$` | `a.b` | false (`Qa.bE` matches) | compile error |
| `^\p{L}$` / `^\pL$` | `a` | false (`p{L}` matches literally) | true |
| `^\u{41}$` | `A` | false (`u` repeated 41 times) | true |
| `^\x{41}$` | `A` | false | true |
| `^\cJ$` | `\n` | true | compile error |
| `^\0$`, `^\01$`, `^\8$`, `^(a)\|\2$`, `^\k$` | `\0`,`\x01`,`8`,`a`,`k` | true (legacy octal / identity) | compile error |
| `^[[:alpha:]]$` | `a` / `[]` | false / true | true / false (POSIX class) |
| `^a{,3}$` | `aa` / `a{,3}` | false / true (literal) | true / false |
| `^{$`, `^a{$`, `^x{1,2$` | literal | true | = |
| `^[^]$` | `a` | true (any char) | compile error |
| `^[]a$` | `a` | false (empty class) | compile error |
| `^[]a]$` | `a`, `]` | false, false (`[]` then `a]`) | true, true |
| `^[a-\d]$` | `-` | true | compile error |
| `a**` | – | compile error | compile error (onig mode: accepted) |
| `(\w+\s?)+\1$`-style blowup | `a…ab` | timeout error after 1 min | `BacktrackLimitExceeded` after about 36 ms |

### Recommended fancy-regex construction

```rust
let re = fancy_regex::RegexBuilder::new(&ecma_to_fancy(pattern)?) // see below
    .backtrack_limit(1_000_000)   // the default; a limit error is handled like a regexp2 timeout
    .build()?;                    // keep unicode_mode at its default (true)
// match: re.is_match(name) -> Err(_) => log + return false   (allowdenylist.go:125-128,169-171)
```

- **Do not use `unicode_mode(false)`**. With `&str` input it makes bare `.`, `\W`, `\S`, `\D`
  and `\p{..}` fail to compile (`$CR/fancy-regex-0.20.2/src/lib.rs:772-805`). Verified: `kube_pod_.*`
  becomes a compile error. Changing it inline (`(?-u:\w)`) is rejected with "Changing Unicode mode inline is
  not supported".
- `oniguruma_mode(true)` (`lib.rs:1083`) does not help. There are still 37 differences: it fixes `\cJ` but newly accepts `a**`.
- Haystacks are Prometheus metric names (`[a-zA-Z_:][a-zA-Z0-9_:]*`), so the differences in Unicode `\d \w \s \b .`
  can never change a result. Only the **syntax** differences matter. Implement `ecma_to_fancy` as a
  small tokenizer that aims to reproduce the regexp2 results above:
  - `\` followed by a letter outside `dDwWsSbBfnrtvcxuk` (non-`/u` Annex-B identity escape) becomes the literal letter.
    This covers `\z \A \Z \G \h \e \a \K \N \R \X \Q \E \p \P`.
  - `\xHH` stays as it is. `\x{…}` becomes the literal `x{…}`. `\uHHHH` becomes `\x{HHHH}`. `\u{…}` becomes the literal `u` with quantifier `{…}`.
  - `\0`, `\0NN`, and `\N` with N greater than the number of groups become legacy octal or a literal digit. `\cX` becomes the control character.
  - `{` that does not start a valid `{n}`, `{n,}` or `{n,m}` becomes `\{`. This covers `a{,3}` and `{` alone.
  - Inside a class: `[]` matches nothing (use `(?!)`), `[^]` matches any character (`[\s\S]`), `]` as the first
    character closes the class, `[` is a literal (so `[[:alpha:]]` turns into `[\[:alph]` followed by `\]`), and `[a-\d]`
    becomes `[a\-0-9]`.
  - Reject `(?P<`, duplicate group names, and possessive `*+ ++ ?+ }+`, because regexp2 rejects them.
  - If the port cannot implement the translator, the next best option is to **reject** any pattern that contains one of these
    constructs and give a clear startup error. That is better than silently matching differently.

### Remaining differences after that recipe
1. Runtime limits differ: regexp2 uses a 1-minute wall-clock timeout plus the 100 000-slot stack. fancy-regex counts backtracking
   steps, and patterns without fancy features are handed to the `regex` DFA, so they never blow up. When both engines error, the outcome is the same (family excluded),
   but fancy-regex reaches the error state roughly 1000× sooner, and on different patterns.
2. regexp2 ECMAScript accepts extensions that standard JS does not: inline `(?imsx)`, `(?#…)`, `(?>…)`
   (all verified). fancy-regex accepts the same ones, so they need no translation.
3. Case-insensitive matching with `(?i)` uses ECMAScript legacy folding in regexp2 (README.md:130). The difference only shows on
   non-ASCII input, so it is irrelevant for metric names.

## 4. Go `regexp` (RE2) vs Rust `regex` (opt-in list)

Diff of the same 83 vectors, Go `regexp.Compile` against `regex::Regex::new` (18 differences):

| Construct | Go RE2 | Rust `regex` |
|---|---|---|
| `\d \w \s \b` | ASCII (`$GOROOT/src/regexp/syntax/perl_groups.go`) | Unicode: `٣`, `é` and NBSP match, `\bpod\b` on `épodé` is false |
| `\s` vs `\v` | `\v` not in `\s` | in `\s` |
| `a{,3}`, `{`, `a{`, `x{1,2` | literal | **compile error** |
| `a**`, `a++` | compile error | accepted |
| `\u0041`, `\u{41}` | compile error | `A` |
| `\0`, `\01` | octal NUL / `\x01` | compile error ("backreferences are not supported") |
| `\Qa.b\E` | literal `a.b` | compile error |
| `(?x)` | compile error | accepted |
| duplicate names `(?<a>x)(?<a>y)` | accepted (Go 1.27) | compile error |

`.` already agrees: both match `\r` and U+2028, and neither matches `\n`. `\pL`, `(?P<n>)`, `(?U)` and `[[:alpha:]]` also agree.
Recommendation: `regex::Regex::new` with default Unicode settings. As in section 3, the haystack is a metric name, so the
Perl-class differences cannot change a result. Either pre-reject the syntax-only differences in the table above
(`{,n}`, lone `{`, `\Q..\E`, `\0`, possessive, `(?x)`, `\u`), or accept the divergence and document it.
`regex::RegexBuilder::unicode(false)` is **not** an option: bare `.` then fails with "pattern can match
invalid UTF-8" (verified), while `(?-u:\w)` inline works in `regex`, unlike fancy-regex.

## 5. Wildcard allowlist (`internal/store/utils.go`)

- `expandWildcard(pattern, limit)` splits on `*`, applies `regexp.QuoteMeta` to each literal, joins the parts with `.*`, and wraps the result in `^…$`
  (`utils.go:350-365`). `limit = MaxPartialWildcardsPerLabel = 1` (`pkg/options/types.go:35`).
  A pattern with more than 1 `*` is rejected *before* expansion (`utils.go:374-377`), so the truncating
  `break` (`:360-362`) is unreachable in production. Go check: `expandWildcard("a*b*c",1)` = `^a.*b$`.
- A plain `*` is "all keys" only at index 0. At any other position it is skipped (`utils.go:296-301`).
  `kubectl.kubernetes.io/last-applied-configuration` is never matched by any wildcard (`utils.go:49-51,317`).
- `QuoteMeta` escapes exactly `\.+*?()|[]{}^$` (`$GOROOT/src/regexp/regexp.go:692-700`).
  `regex::escape` additionally escapes `# & - ~` (`$CR/regex-syntax-0.8.12/src/lib.rs:262-268`). The text differs,
  but the compiled regexes are equivalent, and either output compiles in both Rust engines. The pattern cache is
  keyed by the expanded string (`utils.go:378-384`), which is not observable.
- Bad patterns are cached as `nil` and only logged once (`utils.go:386-390`). Port this as `Option<Regex>` in the cache.

| allowlist entry | expanded (Go) | key → result |
|---|---|---|
| `app.kubernetes.io/*` | `^app\.kubernetes\.io/.*$` | `app.kubernetes.io/name` true, `appXkubernetes.io/name` false, `app.kubernetes.io/` true, `x.app.kubernetes.io/name` false |
| `*/name` | `^.*/name$` | `app.kubernetes.io/name` true, `name` false, `/name` true |
| `team*` | `^team.*$` | `team` true, `team-a` true, `a-team` false |
| `a*b*c` | rejected (2 wildcards) | – |

QuoteMeta vectors: `app.kubernetes.io/name` → `app\.kubernetes\.io/name`; `a-b_c~d#e&f` → unchanged
(Rust: `a\-b_c\~d\#e\&f`); `x*y` → `x\*y`; `é` → `é`.

## 6. Semantic traps
1. **`\z`, `\A`, `\Z`, `\G` are literal letters in regexp2 ECMAScript** (`$GM/.../regexp2/v2@v2.13.1/syntax/parser.go:1471-1473`:
   they are only anchors when `Unicode` is set). In fancy-regex they are anchors. So `^kube_pod_info\z` in Go never matches
   a real metric name, but in Rust it does.
2. More identity escapes with a different meaning in fancy-regex: `\h \e \a \K \N \R`. `\p{L}` is literal `p{L}` in regexp2.
3. `[[:alpha:]]`, `a{,3}`, `[]`, `[^]`, `[]a]` parse differently. See the table in section 3.
4. A match error, whether timeout or stack limit, **excludes** the family even in **denylist** mode (`allowdenylist.go:134-138` is
   skipped because `IsIncluded` returns early with the error). A pathological pattern costs a full 1-minute stall per
   generator at startup in Go.
5. `MatchString` is an unanchored search. Users must anchor themselves: `kube_pod` matches `a_kube_pod`.
   Use `is_match`, never a full-match API.
6. `$` without `(?m)` means end of input in all four engines (`a$` vs `a\n` is false everywhere). There is no PCRE-style
   "before the final newline".
7. Commas split patterns (`types.go:56`), and surrounding whitespace is trimmed (`:57`).
8. `DefaultMatchTimeout` is a process-global set inside `New()` under `sync.Once`. Any regexp2 compile that happens before
   that call has an infinite timeout. This does not happen in KSM, but don't copy it.
9. Opt-in (Go RE2) and allow/deny (ECMAScript) use **different dialects** for the same kind of input. For example, lookahead works
   in `--metric-denylist` and is a compile error in `--metric-opt-in-list`. The port must keep two engines (or two
   validators).

## 7. Concrete test vectors (Go-verified)
`pattern | input → regexp2 ECMAScript MatchString`:
```
kube_pod_.*        | kube_pod_info → true ; xkube_pod_info → true ; kube_node_info → false
^kube_pod_info$    | kube_pod_info_x → false
^kube_(?!pod_).*   | kube_pod_info → false ; kube_node_info → true
(?<!kube_)pod      | kube_pod → false ; x_pod → true
(?<=a+)b           | aab → true ; b → false
^a\z               | a → false ; az → true
^\Aa               | a → false ; Aa → true
^\h$               | h → true
^[[:alpha:]]$      | a → false ; [] → true
^a{,3}$            | a{,3} → true ; aa → false
^\u{41}$           | A → false
^\p{L}$            | p{L} → true ; a → false
^(?P<n>ku)$        | compile error ;   ^a++$ → compile error ;   * → compile error
(a+)+$             | "aaaaaaaaaaaaaaaaaaaaaaaaaaaaab" → ERR match timeout after 1m0s
```
Go `regexp` (opt-in) vectors: `^\s$` on NBSP → false. `^a{,3}$` on `a{,3}` → true. `^\Qa.b\E$` on `a.b`
→ true. `^\0$` on `"\x00"` → true. `(?x) a b` → compile error. `^kube_(?!pod_).*` → compile error.
