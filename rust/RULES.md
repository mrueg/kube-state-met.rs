# Port rules

Behavior the port must keep exactly, learned from corrections. Every agent prompt rust-anything writes includes these rules. Edit them by hand, or add one with `rust-anything rules --add`.

- Allow/deny-list regexes follow regexp2 ECMAScript (non-Unicode) syntax: escapes like \z \A \Z \G \h \p{L} \x{41} are literal characters there; translate patterns before fancy-regex, see docs/port/research/regex.md
- CronJob next schedule is computed from lastScheduleTime or creationTimestamp (never now), in spec.timeZone or the process local zone; unreachable schedules give Go's zero time -62135596800 (docs/port/research/cron.md)
- Go zero values: fields k8s-openapi makes Option but Go stores by value default to Go's zero value; mirror Go nil checks only for real Go pointers
