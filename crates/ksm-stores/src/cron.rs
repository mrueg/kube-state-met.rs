//! Port of the parts of `github.com/netresearch/go-cron` v0.19.0 that
//! `internal/store/cronjob.go` uses: its parser configured as
//! `Minute | Hour | Dom | Month | Dow | Descriptor | RobfigCompat`
//! (descriptors, `@every`, `TZ=`/`CRON_TZ=` prefixes, lenient steps, day of
//! month OR day of week, DST gaps skipped) and `Schedule.Next`.
//!
//! Times are Unix nanoseconds (`i128`); wall-clock fields are computed in the
//! schedule's zone the way Go's `time` package does, including Go's
//! `time.Date` choice of offset around DST transitions and
//! `Time.ZoneBounds`.
//!
//! Known divergence: past the transitions stored in a TZif file, Go derives
//! zone bounds from the POSIX TZ rule and reports artificial bounds at UTC
//! year boundaries. This port only knows the real transitions. That only
//! matters for zones whose offset is not a whole number of hours and that
//! observe DST (e.g. `Australia/Adelaide`) in the hour around New Year UTC.

use std::sync::OnceLock;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use ksm_core::gostd::{civil_from_days, days_from_civil, parse_duration};

const NS: i128 = 1_000_000_000;
const MINUTE_NS: i128 = 60 * NS;
const HOUR_NS: i128 = 60 * MINUTE_NS;
const DAY_SECS: i64 = 86_400;

/// Unix seconds of Go's zero `time.Time`, which `Next` returns when no time
/// matches within the search limit.
pub const GO_ZERO_TIME_UNIX: i64 = -62_135_596_800;

/// Bit 63 marks a field that was written as `*`.
const STAR_BIT: u64 = 1 << 63;
/// Sunday written as 7, folded into bit 0.
const DOW_BIT7: u64 = 1 << 7;
/// Spec length limit (go-cron's `MaxSpecLength`).
const MAX_SPEC_LENGTH: usize = 1024;
/// How many years `Next` searches (go-cron's `defaultSearchYears`).
const SEARCH_YEARS: i64 = 5;
/// The minimum `@every` interval (go-cron's default, one second).
const MIN_EVERY_NS: i64 = 1_000_000_000;

/// Field bounds (go-cron's `bounds`). All fields of this parser wrap around
/// and accept lenient steps (`LenientSteps`).
#[derive(Clone, Copy)]
struct Bounds {
    min: u64,
    max: u64,
    names: &'static [(&'static str, u64)],
    /// Nonzero: an `N/step` range stops at this value (`DowStepToSaturday`).
    step_end: u64,
}

const SECONDS: Bounds = Bounds {
    min: 0,
    max: 59,
    names: &[],
    step_end: 0,
};
const MINUTES: Bounds = SECONDS;
const HOURS: Bounds = Bounds {
    min: 0,
    max: 23,
    names: &[],
    step_end: 0,
};
const DOM: Bounds = Bounds {
    min: 1,
    max: 31,
    names: &[],
    step_end: 0,
};
const MONTHS: Bounds = Bounds {
    min: 1,
    max: 12,
    names: &[
        ("jan", 1),
        ("feb", 2),
        ("mar", 3),
        ("apr", 4),
        ("may", 5),
        ("jun", 6),
        ("jul", 7),
        ("aug", 8),
        ("sep", 9),
        ("oct", 10),
        ("nov", 11),
        ("dec", 12),
    ],
    step_end: 0,
};
const DOW: Bounds = Bounds {
    min: 0,
    max: 7,
    names: &[
        ("sun", 0),
        ("mon", 1),
        ("tue", 2),
        ("wed", 3),
        ("thu", 4),
        ("fri", 5),
        ("sat", 6),
    ],
    step_end: 0,
};
/// Day-of-week bounds with `DowStepToSaturday`.
const DOW_PARSE: Bounds = Bounds { step_end: 6, ..DOW };

/// A parsed schedule (go-cron's `Schedule` implementations this parser
/// returns).
#[derive(Clone, Debug)]
pub enum Schedule {
    Spec(SpecSchedule),
    /// `@every` / `@every_start`: a fixed delay in nanoseconds.
    ConstantDelay(i64),
    /// `@every_aligned`: an interval in nanoseconds on the epoch grid.
    Aligned(i64),
    /// `@triggered`, `@manual`, `@none`: never runs.
    Triggered,
}

/// Where a spec schedule reads its wall clock.
#[derive(Clone, Debug)]
pub enum Location {
    /// Go's `time.Local`.
    Local,
    Zone(TimeZone),
}

/// go-cron's `SpecSchedule` for this parser's options.
#[derive(Clone, Debug)]
pub struct SpecSchedule {
    second: u64,
    minute: u64,
    hour: u64,
    dom: u64,
    month: u64,
    dow: u64,
    location: Location,
    dow_or_dom: bool,
}

/// kube-state-metrics' `parseSchedule`: a `timeZone` becomes a `CRON_TZ=`
/// prefix.
pub fn parse_schedule(schedule: &str, time_zone: Option<&str>) -> Result<Schedule, String> {
    let spec = match time_zone {
        Some(tz) => format!("CRON_TZ={tz} {schedule}"),
        None => schedule.to_owned(),
    };
    parse(&spec).map_err(|e| format!("failed to parse cron job schedule '{spec}': {e}"))
}

/// `cronParser.Parse` of kube-state-metrics.
pub fn parse(spec: &str) -> Result<Schedule, String> {
    if spec.is_empty() {
        return Err("empty spec string".into());
    }
    if spec.len() > MAX_SPEC_LENGTH {
        return Err(format!("spec too long: {} > {MAX_SPEC_LENGTH}", spec.len()));
    }
    let (location, spec) = parse_timezone(spec)?;
    if spec.starts_with('@') {
        return parse_descriptor(spec, location);
    }
    let fields: Vec<&str> = spec.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!(
            "expected exactly 5 fields, found {}: {fields:?}",
            fields.len()
        ));
    }
    // The seconds field defaults to "0".
    let fields = ["0", fields[0], fields[1], fields[2], fields[3], fields[4]];
    if contains_hash_expr(&fields) {
        return Err("h expressions require hash option to be enabled".into());
    }
    let second = get_field(fields[0], SECONDS)?;
    let minute = get_field(fields[1], MINUTES)?;
    let hour = get_field(fields[2], HOURS)?;
    let month = get_field(fields[4], MONTHS)?;
    let dom = parse_dom_field(fields[3])?;
    if fields[5].contains('#') {
        return Err("#n/#L syntax requires DowNth or DowLast option to be enabled".into());
    }
    let dow = normalize_dow(get_field(fields[5], DOW_PARSE)?);
    Ok(Schedule::Spec(SpecSchedule {
        second,
        minute,
        hour,
        dom,
        month,
        dow,
        location,
        dow_or_dom: true,
    }))
}

/// `parseTimezone`.
fn parse_timezone(spec: &str) -> Result<(Location, &str), String> {
    if !spec.starts_with("TZ=") && !spec.starts_with("CRON_TZ=") {
        return Ok((Location::Local, spec));
    }
    let Some(i) = spec.find(' ') else {
        return Err(format!("missing fields after timezone in spec {spec:?}"));
    };
    let eq = spec.find('=').unwrap_or(0);
    let mut tz_name = spec.get(eq + 1..i).unwrap_or("");
    let b = tz_name.as_bytes();
    if b.len() >= 2
        && ((b[0] == b'"' && b[b.len() - 1] == b'"') || (b[0] == b'\'' && b[b.len() - 1] == b'\''))
    {
        tz_name = &tz_name[1..tz_name.len() - 1];
    }
    validate_timezone(tz_name).map_err(|e| format!("invalid timezone {tz_name:?}: {e}"))?;
    let location =
        load_location(tz_name).ok_or_else(|| format!("unknown time zone {tz_name:?}"))?;
    let remaining = spec[i..].trim();
    if remaining.is_empty() {
        return Err(format!("missing fields after timezone {tz_name:?}"));
    }
    Ok((location, remaining))
}

/// `validateTimezone`.
fn validate_timezone(tz: &str) -> Result<(), String> {
    if tz.is_empty() {
        return Err("empty timezone string".into());
    }
    if tz.len() > 64 {
        return Err("timezone string too long (max 64 chars)".into());
    }
    match tz
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+' | ':')))
    {
        Some((i, c)) => Err(format!(
            "invalid character {c:?} at position {i} in timezone"
        )),
        None => Ok(()),
    }
}

/// Go's `time.LoadLocation` (names are case sensitive there).
fn load_location(name: &str) -> Option<Location> {
    match name {
        "" | "UTC" => return Some(Location::Zone(TimeZone::UTC)),
        "Local" => return Some(Location::Local),
        _ => {}
    }
    if name.starts_with('/') || name.starts_with('\\') || name.contains("..") {
        return None;
    }
    lookup_zone(name).map(Location::Zone)
}

/// A zone from the tz database, by its exact (case-sensitive) name.
fn lookup_zone(name: &str) -> Option<TimeZone> {
    let tz = jiff::tz::db().get(name).ok()?;
    (tz.iana_name() == Some(name)).then_some(tz)
}

/// Go's `time.Local`: `$TZ` (unset: `/etc/localtime`, empty or unknown: UTC).
pub fn local_zone() -> &'static TimeZone {
    static LOCAL: OnceLock<TimeZone> = OnceLock::new();
    LOCAL.get_or_init(|| match std::env::var_os("TZ") {
        None => TimeZone::try_system().unwrap_or(TimeZone::UTC),
        Some(tz) => {
            let tz = tz.to_str().unwrap_or("");
            let tz = tz.strip_prefix(':').unwrap_or(tz);
            if tz.starts_with('/') {
                std::fs::read(tz)
                    .ok()
                    .and_then(|data| TimeZone::tzif(tz, &data).ok())
                    .unwrap_or(TimeZone::UTC)
            } else if !tz.is_empty() && tz != "UTC" {
                lookup_zone(tz).unwrap_or(TimeZone::UTC)
            } else {
                TimeZone::UTC
            }
        }
    })
}

/// `containsHashExpr`.
fn contains_hash_expr(fields: &[&str]) -> bool {
    fields
        .iter()
        .any(|f| f.split(',').any(|part| part.starts_with('H')))
}

/// `parseDomField` without `DomL`/`DomW`.
fn parse_dom_field(field: &str) -> Result<u64, String> {
    // Go upper-cases the field and looks for L or W; only l/L/w/W map there.
    if field.contains(['l', 'L', 'w', 'W']) {
        return Err("extended day-of-month syntax requires DomL option (for L, L-n) or DomW option (for nW, LW) to be enabled".into());
    }
    get_field(field, DOM)
}

/// `NormalizeDOW`: Sunday as 7 becomes Sunday as 0.
fn normalize_dow(bits: u64) -> u64 {
    if bits & DOW_BIT7 != 0 {
        (bits | 1) & !DOW_BIT7
    } else {
        bits
    }
}

/// `getFieldWithHash` with hashes disabled: comma-separated ranges, empty
/// parts skipped (`strings.FieldsFunc`).
fn get_field(field: &str, r: Bounds) -> Result<u64, String> {
    let mut bits = 0;
    for expr in field.split(',').filter(|e| !e.is_empty()) {
        if expr.starts_with('H') {
            return Err("h expressions require hash option to be enabled".into());
        }
        bits |= get_range(expr, r)?;
    }
    Ok(bits)
}

/// `getRange`.
fn get_range(expr: &str, r: Bounds) -> Result<u64, String> {
    let range_and_step: Vec<&str> = expr.split('/').collect();
    let low_and_high: Vec<&str> = range_and_step[0].split('-').collect();
    let single_digit = low_and_high.len() == 1;

    let (start, mut end, mut extra) = if low_and_high[0] == "*" || low_and_high[0] == "?" {
        (r.min, r.max, STAR_BIT)
    } else {
        let start = parse_int_or_name(low_and_high[0], r.names)?;
        match low_and_high.len() {
            1 => (start, start, 0),
            2 => (start, parse_int_or_name(low_and_high[1], r.names)?, 0),
            _ => return Err(format!("too many hyphens: {range_and_step:?}")),
        }
    };

    let mut dropped = 0;
    let step = match range_and_step.len() {
        1 => 1,
        2 => {
            let step = must_parse_int(range_and_step[1])?;
            // "N/step" means "N-max/step".
            if single_digit {
                end = r.max;
                if r.step_end != 0 && extra == 0 && start <= r.step_end {
                    dropped = get_bits(r.step_end + 1, r.max, 1);
                }
            }
            if step > 1 {
                extra = 0;
            }
            step
        }
        _ => return Err(format!("too many slashes: {expr:?}")),
    };

    validate_range_params(start, end, step, r, expr)?;

    // All fields wrap around (22-2 for hours).
    if start > end {
        return Ok(get_wraparound_bits(start, end, step, r) | extra);
    }
    Ok((get_bits(start, end, step) & !dropped) | extra)
}

/// `validateRangeParams` with `LenientSteps` and wrapping fields.
fn validate_range_params(
    start: u64,
    end: u64,
    step: u64,
    r: Bounds,
    expr: &str,
) -> Result<(), String> {
    if start < r.min {
        return Err(format!(
            "beginning of range ({start}) below minimum ({}): {expr:?}",
            r.min
        ));
    }
    if start > r.max {
        return Err(format!(
            "beginning of range ({start}) above maximum ({}): {expr:?}",
            r.max
        ));
    }
    if end < r.min {
        return Err(format!(
            "end of range ({end}) below minimum ({}): {expr:?}",
            r.min
        ));
    }
    if end > r.max {
        return Err(format!(
            "end of range ({end}) above maximum ({}): {expr:?}",
            r.max
        ));
    }
    if step == 0 {
        return Err(format!("step of range must be a positive number: {expr:?}"));
    }
    Ok(())
}

/// `parseIntOrName`.
fn parse_int_or_name(expr: &str, names: &[(&str, u64)]) -> Result<u64, String> {
    if !names.is_empty() {
        let lower = ksm_core::gostr::to_lower(expr);
        if let Some((_, v)) = names.iter().find(|(n, _)| *n == lower) {
            return Ok(*v);
        }
    }
    must_parse_int(expr)
}

/// `mustParseInt`: Go's `strconv.Atoi`, negative numbers rejected.
fn must_parse_int(expr: &str) -> Result<u64, String> {
    let num: i64 = expr
        .parse()
        .map_err(|_| format!("failed to parse int from {expr:?}"))?;
    u64::try_from(num).map_err(|_| format!("negative number ({num}) not allowed: {expr:?}"))
}

/// `getBits`: the bits from `low` to `high`, every `step`.
fn get_bits(low: u64, high: u64, step: u64) -> u64 {
    let shl = |n: u64| {
        u32::try_from(n)
            .ok()
            .and_then(|n| u64::MAX.checked_shl(n))
            .unwrap_or(0)
    };
    if step == 1 {
        return !shl(high + 1) & shl(low);
    }
    let mut bits = 0;
    let mut i = low;
    while i <= high {
        bits |= bit(i);
        match i.checked_add(step) {
            Some(n) => i = n,
            None => break,
        }
    }
    bits
}

fn bit(v: u64) -> u64 {
    u32::try_from(v)
        .ok()
        .and_then(|v| 1u64.checked_shl(v))
        .unwrap_or(0)
}

/// `dowCycle`: day-of-week ranges walk the seven distinct days.
fn dow_cycle(start: u64, end: u64, mut r: Bounds) -> (u64, u64, Bounds) {
    if !(r.min == DOW.min && r.max == DOW.max) {
        return (start, end, r);
    }
    let end = if start == 0 && end == 7 { 6 } else { end };
    r.max = 6;
    (start % 7, end % 7, r)
}

/// `getWraparoundBits`.
fn get_wraparound_bits(start: u64, end: u64, step: u64, r: Bounds) -> u64 {
    let (mut start, mut end, mut r) = (start, end, r);
    if step > 1 {
        (start, end, r) = dow_cycle(start, end, r);
        if start <= end {
            return get_bits(start, end, step);
        }
    }
    let range_size = r.max - r.min + 1;
    let extended_end = end + range_size;
    let mut bits = 0;
    let mut i = start;
    while i <= extended_end {
        let val = if i > r.max {
            (i - r.min) % range_size + r.min
        } else {
            i
        };
        bits |= bit(val);
        match i.checked_add(step) {
            Some(n) => i = n,
            None => break,
        }
    }
    bits
}

/// `all`: every value of the bounds plus the star bit.
fn all(r: Bounds) -> u64 {
    get_bits(r.min, r.max, 1) | STAR_BIT
}

/// `parseDescriptor`.
fn parse_descriptor(descriptor: &str, location: Location) -> Result<Schedule, String> {
    let all_dow = normalize_dow(all(DOW));
    let spec = |hour, dom, month, dow| {
        Ok(Schedule::Spec(SpecSchedule {
            second: 1 << SECONDS.min,
            minute: 1 << MINUTES.min,
            hour,
            dom,
            month,
            dow,
            location: location.clone(),
            dow_or_dom: false,
        }))
    };
    match descriptor {
        "@yearly" | "@annually" => {
            return spec(1 << HOURS.min, 1 << DOM.min, 1 << MONTHS.min, all_dow);
        }
        "@monthly" => return spec(1 << HOURS.min, 1 << DOM.min, all(MONTHS), all_dow),
        "@weekly" => return spec(1 << HOURS.min, all(DOM), all(MONTHS), 1 << DOW.min),
        "@daily" | "@midnight" => return spec(1 << HOURS.min, all(DOM), all(MONTHS), all_dow),
        "@hourly" => return spec(all(HOURS), all(DOM), all(MONTHS), all_dow),
        "@triggered" | "@manual" | "@none" => return Ok(Schedule::Triggered),
        _ => {}
    }
    for kw in ["@every_aligned ", "@every_start ", "@every "] {
        let Some(rest) = descriptor.strip_prefix(kw) else {
            continue;
        };
        let duration = parse_duration(rest)
            .map_err(|e| format!("failed to parse duration {descriptor:?}: {e}"))?;
        // EveryRoundUp: shorter intervals are rounded up, not rejected.
        let delay = every_with_min(duration);
        return Ok(if kw == "@every_aligned " {
            Schedule::Aligned(delay)
        } else {
            Schedule::ConstantDelay(delay)
        });
    }
    Err(format!("unrecognized descriptor: {descriptor:?}"))
}

/// `EveryWithMin(d, time.Second)`: at least a second, whole seconds.
fn every_with_min(duration: i64) -> i64 {
    let d = duration.max(MIN_EVERY_NS);
    d - d % 1_000_000_000
}

impl Schedule {
    /// `Next(t).Unix()` for `t` in Unix nanoseconds; Go's zero time (no
    /// match) gives [`GO_ZERO_TIME_UNIX`].
    pub fn next_unix(&self, t: i128) -> i64 {
        let next = match self {
            Schedule::Spec(s) => {
                let zone = match &s.location {
                    Location::Local => local_zone(),
                    Location::Zone(z) => z,
                };
                Search { s, z: Zone(zone) }.next_existing(t)
            }
            Schedule::ConstantDelay(delay) => Some(t - t.rem_euclid(NS) + i128::from(*delay)),
            Schedule::Aligned(interval) => {
                let n = i128::from(*interval);
                Some(t + n - t.rem_euclid(n))
            }
            Schedule::Triggered => None,
        };
        next.map_or(GO_ZERO_TIME_UNIX, unix_secs)
    }
}

/// Unix seconds of a time (Go `Time.Unix`, rounding down).
fn unix_secs(t: i128) -> i64 {
    let s = t.div_euclid(NS);
    i64::try_from(s).unwrap_or(if s < 0 { i64::MIN } else { i64::MAX })
}

/// A zone's offsets and transitions, as Go's `Location.lookup` sees them.
#[derive(Clone, Copy)]
struct Zone<'a>(&'a TimeZone);

fn ts(sec: i64) -> Timestamp {
    let sec = sec.clamp(Timestamp::MIN.as_second(), Timestamp::MAX.as_second());
    Timestamp::from_second(sec).unwrap_or(Timestamp::UNIX_EPOCH)
}

impl Zone<'_> {
    /// UTC offset in seconds at Unix second `sec`.
    fn offset(self, sec: i64) -> i64 {
        i64::from(self.0.to_offset(ts(sec)).seconds())
    }

    /// Start of the zone period holding `sec` (a transition at or before it).
    fn start(self, sec: i64) -> Option<i64> {
        self.0
            .preceding(ts(sec.saturating_add(1)))
            .next()
            .map(|t| t.timestamp().as_second())
            .filter(|&s| s <= sec)
    }

    /// End of the zone period holding `sec` (the first transition after it).
    fn end(self, sec: i64) -> Option<i64> {
        self.0
            .following(ts(sec))
            .next()
            .map(|t| t.timestamp().as_second())
            .filter(|&e| e > sec)
    }
}

/// Wall-clock fields of a time in a zone.
#[derive(Clone, Copy)]
struct Parts {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    nanosecond: i64,
    weekday: i64,
    /// Days since 1970-01-01 of the wall date.
    days: i64,
}

/// The search of `SpecSchedule.Next` in one zone. Every time it handles is
/// in that zone, as in Go once `prepareTimeForSchedule` converted it.
struct Search<'a> {
    s: &'a SpecSchedule,
    z: Zone<'a>,
}

fn field_matches(value: i64, bits: u64) -> bool {
    u64::try_from(value).is_ok_and(|v| bit(v) & bits != 0)
}

impl Search<'_> {
    fn parts(&self, t: i128) -> Parts {
        let sec = unix_secs(t);
        let nanosecond = i64::try_from(t.rem_euclid(NS)).unwrap_or(0);
        let local = sec.saturating_add(self.z.offset(sec));
        let days = local.div_euclid(DAY_SECS);
        let rem = local.rem_euclid(DAY_SECS);
        let (year, month, day) = civil_from_days(days);
        Parts {
            year,
            month,
            day,
            hour: rem / 3600,
            minute: rem % 3600 / 60,
            second: rem % 60,
            nanosecond,
            weekday: (days + 4).rem_euclid(7),
            days,
        }
    }

    fn offset(&self, t: i128) -> i64 {
        self.z.offset(unix_secs(t))
    }

    /// `Time.ZoneBounds`.
    fn zone_bounds(&self, t: i128) -> (Option<i128>, Option<i128>) {
        let sec = unix_secs(t);
        (
            self.z.start(sec).map(|s| i128::from(s) * NS),
            self.z.end(sec).map(|s| i128::from(s) * NS),
        )
    }

    /// Go's `time.Date(year, month, day, hour, min, sec, 0, loc)`, with
    /// out-of-range fields normalized and Go's choice of offset.
    fn date(&self, year: i64, month: i64, day: i64, hour: i64, min: i64, sec: i64) -> i128 {
        let m0 = month - 1;
        let year = year + m0.div_euclid(12);
        let month = m0.rem_euclid(12) + 1;
        let days = days_from_civil(year, month, 1) + day - 1;
        let mut unix = days * DAY_SECS + hour * 3600 + min * 60 + sec;
        let offset = self.z.offset(unix);
        if offset != 0 {
            let utc = unix - offset;
            let start = self.z.start(unix);
            let end = self.z.end(unix);
            let outside = start.is_some_and(|s| utc < s) || end.is_some_and(|e| utc >= e);
            let offset = if outside { self.z.offset(utc) } else { offset };
            unix -= offset;
        }
        i128::from(unix) * NS
    }

    /// `nextExisting` (with `DSTSkip`, `Next` is exactly this).
    fn next_existing(&self, t: i128) -> Option<i128> {
        // prepareTimeForSchedule: start at the upcoming second.
        let mut t = t + NS - t.rem_euclid(NS);
        let mut added = false;
        let year_limit = self.parts(t).year + SEARCH_YEARS;
        while self.parts(t).year <= year_limit {
            let step;
            (t, added, step) = self.advance_date(t, added);
            match step {
                Step::Wrapped => continue,
                Step::Found => {}
            }
            let wrap;
            (t, added, wrap) = self.advance_time(t, added);
            if !wrap {
                return Some(t);
            }
        }
        None
    }

    /// `advanceDate` (no year field).
    fn advance_date(&self, mut t: i128, mut added: bool) -> (i128, bool, Step) {
        loop {
            let p = self.parts(t);
            if field_matches(p.month, self.s.month) {
                break;
            }
            added = true;
            t = self.date_start(p.year, p.month + 1, 1, t);
            if self.parts(t).year != p.year {
                return (t, added, Step::Wrapped);
            }
        }
        self.advance_day(t, added)
    }

    /// `advanceDay`.
    fn advance_day(&self, mut t: i128, mut added: bool) -> (i128, bool, Step) {
        loop {
            let p = self.parts(t);
            if self.day_matches(p) {
                return (t, added, Step::Found);
            }
            added = true;
            t = self.date_start(p.year, p.month, p.day + 1, t);
            if self.parts(t).month != p.month {
                return (t, added, Step::Wrapped);
            }
        }
    }

    /// `advanceTime`.
    fn advance_time(&self, t: i128, added: bool) -> (i128, bool, bool) {
        let (t, added, wrap) = self.advance_hour(t, added);
        if wrap {
            return (t, added, true);
        }
        let (t, added, wrap) = self.advance_minute(t, added);
        if wrap {
            return (t, added, true);
        }
        let (t, _, wrap) = self.advance_second(t, added);
        (t, added, wrap)
    }

    /// `dayMatches`.
    fn day_matches(&self, p: Parts) -> bool {
        let dom_match = field_matches(p.day, self.s.dom);
        let dow_match = field_matches(p.weekday, self.s.dow);
        if self.s.dow_or_dom {
            if self.s.dom & STAR_BIT > 0 || self.s.dow & STAR_BIT > 0 {
                return dom_match && dow_match;
            }
            return dom_match || dow_match;
        }
        dom_match && dow_match
    }

    /// `advanceHour`.
    fn advance_hour(&self, mut t: i128, mut added: bool) -> (i128, bool, bool) {
        loop {
            let p = self.parts(t);
            if field_matches(p.hour, self.s.hour) {
                return (t, added, false);
            }
            added = true;
            t = self.next_hour_start(t);
            if self.parts(t).day != p.day {
                return (t, added, true);
            }
        }
    }

    /// `advanceMinute`.
    fn advance_minute(&self, mut t: i128, mut added: bool) -> (i128, bool, bool) {
        let mut change: Option<i128> = None;
        let mut looked = false;
        let mut minute = self.parts(t).minute;
        while !field_matches(minute, self.s.minute) {
            added = true;
            if looked {
                t += MINUTE_NS;
            } else {
                change = self.zone_end_after(t);
                looked = true;
                t = self.next_minute_start(t);
            }
            minute = self.parts(t).minute;
            if let Some(c) = change
                && t > c
            {
                return (c, added, true);
            }
            if minute == 0 || change.is_some_and(|c| t >= c) {
                return (t, added, true);
            }
        }
        (t, added, false)
    }

    /// `advanceSecond`.
    fn advance_second(&self, mut t: i128, mut added: bool) -> (i128, bool, bool) {
        let mut sec = self.parts(t).second;
        while !field_matches(sec, self.s.second) {
            if !added {
                added = true;
                t -= t.rem_euclid(NS);
            }
            t += NS;
            let prev = sec;
            sec = self.parts(t).second;
            if sec <= prev {
                return (t, added, true);
            }
        }
        (t, added, false)
    }

    /// `nextMinuteStart`.
    fn next_minute_start(&self, t: i128) -> i128 {
        let p = self.parts(t);
        t + MINUTE_NS - i128::from(p.second) * NS - i128::from(p.nanosecond)
    }

    /// `zoneEndAfter`: the first zone change after `t`.
    fn zone_end_after(&self, t: i128) -> Option<i128> {
        self.zone_bounds(t).1.filter(|&e| e > t)
    }

    /// `nextHourStart`.
    fn next_hour_start(&self, t: i128) -> i128 {
        let p = self.parts(t);
        let want = (p.hour + 1) % 24;
        if p.minute == 0 && p.second == 0 && p.nanosecond == 0 {
            let next = t + HOUR_NS;
            let n = self.parts(next);
            if n.hour == want && n.minute == 0 {
                return next;
            }
        }
        let next = self.next_full_hour(t, p, want);
        if let Some(end) = self.zone_end_after(t)
            && end < next
            && self.parts(end).hour != p.hour
        {
            return end;
        }
        next
    }

    /// `nextFullHour`.
    fn next_full_hour(&self, t: i128, p: Parts, want: i64) -> i128 {
        let next = self.date(p.year, p.month, p.day, p.hour + 1, 0, 0);
        let n = self.parts(next);
        if n.hour == want && n.minute == 0 {
            let first = self.earlier_copy(next);
            if first > t {
                return first;
            }
            if next > t {
                return next;
            }
            let second = self.later_copy(next);
            if second > t {
                return second;
            }
        }
        t + HOUR_NS
    }

    /// `earlierCopy`.
    fn earlier_copy(&self, t: i128) -> i128 {
        let Some(start) = self.zone_bounds(t).0 else {
            return t;
        };
        let before = self.offset(start - 1);
        let after = self.offset(start);
        let repeated = i128::from(before - after) * NS;
        if repeated <= 0 || t >= start + repeated {
            return t;
        }
        t - repeated
    }

    /// `laterCopy`.
    fn later_copy(&self, t: i128) -> i128 {
        let Some(end) = self.zone_bounds(t).1 else {
            return t;
        };
        let before = self.offset(t);
        let after = self.offset(end);
        let repeated = i128::from(before - after) * NS;
        if repeated <= 0 || t < end - repeated {
            return t;
        }
        t + repeated
    }

    /// `dateStart`.
    fn date_start(&self, y: i64, mo: i64, d: i64, after: i128) -> i128 {
        let m0 = mo - 1;
        let want = days_from_civil(y + m0.div_euclid(12), m0.rem_euclid(12) + 1, 1) + d - 1;
        let m = self.date(y, mo, d, 0, 0, 0);
        let mp = self.parts(m);
        if mp.days == want && mp.hour == 0 && mp.minute == 0 && mp.second == 0 {
            for c in [self.earlier_copy(m), m, self.later_copy(m)] {
                if c > after {
                    return c;
                }
            }
        }
        let early = m - 48 * HOUR_NS;
        let from = if early > after { early } else { after };
        self.date_change_after(want, from)
    }

    /// `dateChangeAfter`.
    fn date_change_after(&self, want: i64, mut t: i128) -> i128 {
        for _ in 0..64 {
            let next = self.next_midnight_or_change(t);
            if self.parts(next).days >= want {
                return next;
            }
            t = next;
        }
        t + NS
    }

    /// `nextMidnightOrChange`.
    fn next_midnight_or_change(&self, t: i128) -> i128 {
        let p = self.parts(t);
        let mut next = self.date(p.year, p.month, p.day + 1, 0, 0, 0);
        let first = self.earlier_copy(next);
        if first > t {
            next = first;
        }
        if let Some(end) = self.zone_end_after(t)
            && (next <= t || end < next)
        {
            next = end;
        }
        if next <= t {
            next = t + HOUR_NS;
        }
        next
    }
}

/// `searchStep` (the year field is never set, so the search is never
/// exhausted early).
enum Step {
    Found,
    Wrapped,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `parser.Parse(spec)` and `Next(t).Unix()` as printed by go-cron
    /// v0.19.0 (scratch Go program; `None` = parse error).
    const GO_CASES: &[(&str, &str, Option<i64>)] = &[
        (
            "CRON_TZ=Asia/Shanghai 0 */6 * * *",
            "2018-03-11T04:34:56Z",
            Some(1520762400),
        ),
        (
            "CRON_TZ=Asia/Singapore 0 */6 * * *",
            "2018-03-11T04:34:56Z",
            Some(1520762400),
        ),
        (
            "CRON_TZ=UTC 25 * * * *",
            "2018-03-11T11:04:56Z",
            Some(1520767500),
        ),
        (
            "CRON_TZ=UTC 25 * * * *",
            "2018-03-11T11:25:00Z",
            Some(1520771100),
        ),
        (
            "CRON_TZ=UTC 25 * * * *",
            "2018-03-11T11:24:59.5Z",
            Some(1520767500),
        ),
        (
            "CRON_TZ=Europe/Berlin 30 2 * * *",
            "2024-03-30T12:00:00Z",
            Some(1711931400),
        ),
        (
            "CRON_TZ=Europe/Berlin 30 2 * * *",
            "2024-10-26T12:00:00Z",
            Some(1729989000),
        ),
        (
            "CRON_TZ=Europe/Berlin 30 2 * * *",
            "2024-10-27T00:30:00Z",
            Some(1729992600),
        ),
        (
            "CRON_TZ=Europe/Berlin 0 * * * *",
            "2024-10-27T00:10:00Z",
            Some(1729990800),
        ),
        (
            "CRON_TZ=Europe/Berlin 0 * * * *",
            "2024-10-27T01:10:00Z",
            Some(1729994400),
        ),
        (
            "CRON_TZ=Europe/Berlin 0 * * * *",
            "2024-03-31T00:10:00Z",
            Some(1711846800),
        ),
        (
            "CRON_TZ=Europe/Berlin 0 0 * * *",
            "2024-03-30T23:00:00Z",
            Some(1711922400),
        ),
        (
            "CRON_TZ=America/New_York 0 * * * *",
            "2024-11-03T05:30:00Z",
            Some(1730613600),
        ),
        (
            "CRON_TZ=America/New_York 30 1 * * *",
            "2024-11-03T05:30:00Z",
            Some(1730615400),
        ),
        (
            "CRON_TZ=America/New_York 30 1 * * *",
            "2024-11-03T06:30:00Z",
            Some(1730701800),
        ),
        (
            "CRON_TZ=America/New_York 0 2 * * *",
            "2024-03-09T12:00:00Z",
            Some(1710136800),
        ),
        (
            "CRON_TZ=Australia/Lord_Howe */15 * * * *",
            "2024-10-05T15:20:00Z",
            Some(1728142200),
        ),
        (
            "CRON_TZ=Australia/Lord_Howe 0 2 * * *",
            "2024-10-05T12:00:00Z",
            Some(1728226800),
        ),
        (
            "CRON_TZ=Australia/Lord_Howe 45 1 * * *",
            "2024-04-06T12:00:00Z",
            Some(1712414700),
        ),
        (
            "CRON_TZ=Australia/Lord_Howe 45 1 * * *",
            "2024-04-06T14:50:00Z",
            Some(1712416500),
        ),
        (
            "CRON_TZ=Australia/Adelaide 15 * * * *",
            "2024-12-31T23:05:00Z",
            Some(1735688700),
        ),
        (
            "CRON_TZ=Australia/Adelaide 15 * * * *",
            "2030-12-31T23:05:00Z",
            Some(1924991100),
        ),
        (
            "CRON_TZ=Australia/Adelaide 0,15,30,45 * * * *",
            "2040-12-31T13:05:00Z",
            Some(2240572500),
        ),
        (
            "CRON_TZ=Australia/Adelaide 0 * * * *",
            "2040-12-31T13:05:00Z",
            Some(2240573400),
        ),
        (
            "CRON_TZ=Asia/Kathmandu 0 0 * * *",
            "2024-05-01T00:00:00Z",
            Some(1714587300),
        ),
        (
            "CRON_TZ=Asia/Kolkata 0 0 1 jan-mar *",
            "2024-05-01T00:00:00Z",
            Some(1735669800),
        ),
        (
            "CRON_TZ=Pacific/Apia 0 12 * * *",
            "2011-12-29T00:00:00Z",
            Some(1325196000),
        ),
        (
            "CRON_TZ=Pacific/Apia 0 0 * * *",
            "2011-12-29T12:00:00Z",
            Some(1325239200),
        ),
        (
            "CRON_TZ=America/Sao_Paulo 0 0 * * *",
            "2018-11-03T12:00:00Z",
            Some(1541383200),
        ),
        (
            "CRON_TZ=America/Sao_Paulo 0 0 4 11 *",
            "2018-11-03T12:00:00Z",
            Some(1572836400),
        ),
        (
            "CRON_TZ=America/Havana 0 0 * * *",
            "2024-03-09T12:00:00Z",
            Some(1710129600),
        ),
        (
            "CRON_TZ=America/New_York 0 0 13 * 5",
            "2024-01-01T00:00:00Z",
            Some(1704430800),
        ),
        (
            "CRON_TZ=America/New_York 0 0 13 * 5",
            "2024-01-05T06:00:00Z",
            Some(1705035600),
        ),
        (
            "CRON_TZ=UTC 0 0 * * 7",
            "2024-01-01T00:00:00Z",
            Some(1704585600),
        ),
        (
            "CRON_TZ=UTC 0 0 * * 1/2",
            "2024-01-06T00:00:00Z",
            Some(1704672000),
        ),
        (
            "CRON_TZ=UTC 0 0 * * 5-1/2",
            "2024-01-01T00:00:00Z",
            Some(1704412800),
        ),
        (
            "CRON_TZ=UTC 0 0 * * 5-1/2",
            "2024-01-03T00:00:00Z",
            Some(1704412800),
        ),
        (
            "CRON_TZ=UTC 0 0 * * fri-mon",
            "2024-01-02T00:00:00Z",
            Some(1704412800),
        ),
        (
            "CRON_TZ=UTC 0 22-2 * * *",
            "2024-01-02T03:00:00Z",
            Some(1704232800),
        ),
        (
            "CRON_TZ=UTC 0 22-2/3 * * *",
            "2024-01-02T03:00:00Z",
            Some(1704232800),
        ),
        (
            "CRON_TZ=UTC 50-10/5 * * * *",
            "2024-01-02T03:11:00Z",
            Some(1704167400),
        ),
        (
            "CRON_TZ=UTC */60 * * * *",
            "2024-01-02T03:11:00Z",
            Some(1704168000),
        ),
        (
            "CRON_TZ=UTC 5/15 * * * *",
            "2024-01-02T03:51:00Z",
            Some(1704168300),
        ),
        (
            "CRON_TZ=UTC 0 0 ? * MON",
            "2024-01-02T03:51:00Z",
            Some(1704672000),
        ),
        (
            "CRON_TZ=UTC 0 0 1,15 * *",
            "2024-01-02T03:51:00Z",
            Some(1705276800),
        ),
        (
            "CRON_TZ=UTC 0 0 , * *",
            "2024-01-02T03:51:00Z",
            Some(-62135596800),
        ),
        (
            "CRON_TZ=UTC 0 0 30 2 *",
            "2024-01-02T03:51:00Z",
            Some(-62135596800),
        ),
        (
            "CRON_TZ=UTC 0 0 29 2 *",
            "2025-01-02T03:51:00Z",
            Some(1835395200),
        ),
        (
            "CRON_TZ=UTC 0 0 31 * *",
            "2024-04-02T03:51:00Z",
            Some(1717113600),
        ),
        (
            "CRON_TZ=UTC 0 0 * * *-5",
            "2024-04-02T03:51:00Z",
            Some(1712102400),
        ),
        (
            "CRON_TZ=UTC 0 0 * 11-2 *",
            "2024-04-02T03:51:00Z",
            Some(1730419200),
        ),
        (
            "CRON_TZ=UTC 0 0 * * 6-0",
            "2024-04-02T03:51:00Z",
            Some(1712361600),
        ),
        (
            "CRON_TZ=UTC 0 0 * * 7-2/2",
            "2024-04-02T03:51:00Z",
            Some(1712448000),
        ),
        (
            "CRON_TZ=UTC 0 0 * * 0-7/3",
            "2024-04-02T03:51:00Z",
            Some(1712102400),
        ),
        (
            "CRON_TZ=UTC 0 0 * * 6/1",
            "2024-04-02T03:51:00Z",
            Some(1712361600),
        ),
        (
            "CRON_TZ=UTC 0 0 * * +3",
            "2024-04-02T03:51:00Z",
            Some(1712102400),
        ),
        (
            "CRON_TZ=UTC 0 0 * * FRİ",
            "2024-04-02T03:51:00Z",
            Some(1712275200),
        ),
        (
            "CRON_TZ=UTC 0 0 1 */2 *",
            "2024-04-02T03:51:00Z",
            Some(1714521600),
        ),
        (
            "CRON_TZ=Europe/Berlin @daily",
            "2024-03-30T23:30:00Z",
            Some(1711922400),
        ),
        (
            "CRON_TZ=Europe/Berlin @hourly",
            "2024-03-31T00:30:00Z",
            Some(1711846800),
        ),
        (
            "CRON_TZ=Europe/Berlin @weekly",
            "2024-03-30T23:30:00Z",
            Some(1712440800),
        ),
        (
            "CRON_TZ=Europe/Berlin @monthly",
            "2024-03-30T23:30:00Z",
            Some(1711922400),
        ),
        (
            "CRON_TZ=Europe/Berlin @yearly",
            "2024-03-30T23:30:00Z",
            Some(1735686000),
        ),
        (
            "CRON_TZ=Europe/Berlin @annually",
            "2024-03-30T23:30:00Z",
            Some(1735686000),
        ),
        (
            "CRON_TZ=Europe/Berlin @midnight",
            "2024-03-30T23:30:00Z",
            Some(1711922400),
        ),
        (
            "CRON_TZ=UTC @every 90m",
            "2024-03-30T23:30:12.5Z",
            Some(1711846812),
        ),
        (
            "CRON_TZ=UTC @every 1.5s",
            "2024-03-30T23:30:12.5Z",
            Some(1711841413),
        ),
        (
            "CRON_TZ=UTC @every 500ms",
            "2024-03-30T23:30:12.5Z",
            Some(1711841413),
        ),
        (
            "CRON_TZ=UTC @every -5m",
            "2024-03-30T23:30:12.5Z",
            Some(1711841413),
        ),
        (
            "CRON_TZ=UTC @every 1h2m3.9s",
            "2024-03-30T23:30:12.5Z",
            Some(1711845135),
        ),
        (
            "CRON_TZ=UTC @every_start 7m",
            "2024-03-30T23:30:12.5Z",
            Some(1711841832),
        ),
        (
            "CRON_TZ=UTC @every_aligned 7m",
            "2024-03-30T23:30:12.5Z",
            Some(1711841460),
        ),
        (
            "CRON_TZ=UTC @every_aligned 1.5s",
            "2024-03-30T23:30:12.7Z",
            Some(1711841413),
        ),
        (
            "CRON_TZ=UTC @every_aligned 2562047h",
            "2024-03-30T23:30:12.7Z",
            Some(9223369200),
        ),
        (
            "CRON_TZ=UTC @every 2562047h47m16.854775807s",
            "2024-03-30T23:30:12.7Z",
            Some(10935213448),
        ),
        (
            "CRON_TZ=UTC @every 2562047h47m16.854775808s",
            "2024-03-30T23:30:12.7Z",
            None,
        ),
        (
            "CRON_TZ=UTC @every .5h",
            "2024-03-30T23:30:12.7Z",
            Some(1711843212),
        ),
        (
            "CRON_TZ=UTC @every 5.h",
            "2024-03-30T23:30:12.7Z",
            Some(1711859412),
        ),
        (
            "CRON_TZ=UTC @every 1µs",
            "2024-03-30T23:30:12.7Z",
            Some(1711841413),
        ),
        (
            "CRON_TZ=UTC @every 0",
            "2024-03-30T23:30:12.7Z",
            Some(1711841413),
        ),
        ("CRON_TZ=UTC @every 1d", "2024-03-30T23:30:12.7Z", None),
        ("CRON_TZ=UTC @every ", "2024-03-30T23:30:12.7Z", None),
        ("CRON_TZ=UTC @every", "2024-03-30T23:30:12.7Z", None),
        (
            "CRON_TZ=UTC @triggered",
            "2024-03-30T23:30:12.7Z",
            Some(-62135596800),
        ),
        (
            "CRON_TZ=UTC @none",
            "2024-03-30T23:30:12.7Z",
            Some(-62135596800),
        ),
        ("CRON_TZ=UTC @reboot", "2024-03-30T23:30:12.7Z", None),
        (
            "CRON_TZ=UTC  @daily",
            "2024-03-30T23:30:12.7Z",
            Some(1711843200),
        ),
        (
            "CRON_TZ=\"Asia/Tokyo\" 0 0 * * *",
            "2024-03-30T23:30:12.7Z",
            Some(1711897200),
        ),
        (
            "CRON_TZ='Asia/Tokyo' 0 0 * * *",
            "2024-03-30T23:30:12.7Z",
            Some(1711897200),
        ),
        (
            "TZ=Asia/Tokyo 0 0 * * *",
            "2024-03-30T23:30:12.7Z",
            Some(1711897200),
        ),
        (
            "CRON_TZ=US/Pacific 0 0 * * *",
            "2024-03-30T23:30:12.7Z",
            Some(1711868400),
        ),
        (
            "CRON_TZ=Etc/GMT+5 0 0 * * *",
            "2024-03-30T23:30:12.7Z",
            Some(1711861200),
        ),
        (
            "CRON_TZ=Invalid/Zone 0 * * * *",
            "2024-03-30T23:30:12.7Z",
            None,
        ),
        (
            "CRON_TZ=europe/berlin 0 * * * *",
            "2024-03-30T23:30:12.7Z",
            None,
        ),
        ("CRON_TZ=utc 0 * * * *", "2024-03-30T23:30:12.7Z", None),
        (
            "CRON_TZ=Etc/Unknown 0 * * * *",
            "2024-03-30T23:30:12.7Z",
            None,
        ),
        ("CRON_TZ= 0 * * * *", "2024-03-30T23:30:12.7Z", None),
        ("CRON_TZ=Asia/Tokyo", "2024-03-30T23:30:12.7Z", None),
        (
            "CRON_TZ=Asia/Tokyo TZ=UTC 0 * * * *",
            "2024-03-30T23:30:12.7Z",
            None,
        ),
        ("*/0 * * * *", "2024-03-30T23:30:12.7Z", None),
        ("60 * * * *", "2024-03-30T23:30:12.7Z", None),
        ("0 0 L * *", "2024-03-30T23:30:12.7Z", None),
        ("0 0 15W * *", "2024-03-30T23:30:12.7Z", None),
        ("0 0 * * 1#2", "2024-03-30T23:30:12.7Z", None),
        ("0 0 * * 1#L", "2024-03-30T23:30:12.7Z", None),
        ("H * * * *", "2024-03-30T23:30:12.7Z", None),
        ("0 0 * * THU,H", "2024-03-30T23:30:12.7Z", None),
        ("0 0 * *", "2024-03-30T23:30:12.7Z", None),
        ("0 0 * * * *", "2024-03-30T23:30:12.7Z", None),
        ("0 0 0 * *", "2024-03-30T23:30:12.7Z", None),
        ("0 0 * 13 *", "2024-03-30T23:30:12.7Z", None),
        ("0 0 * * 8", "2024-03-30T23:30:12.7Z", None),
        ("0 24 * * *", "2024-03-30T23:30:12.7Z", None),
        ("0 0 * * -1", "2024-03-30T23:30:12.7Z", None),
        ("0 0 * * 1-2-3", "2024-03-30T23:30:12.7Z", None),
        ("0 0 * * 1/2/3", "2024-03-30T23:30:12.7Z", None),
        ("0 0 * * 1/0", "2024-03-30T23:30:12.7Z", None),
        ("0 0 * * /2", "2024-03-30T23:30:12.7Z", None),
        (
            "0 0 * * 99999999999999999999",
            "2024-03-30T23:30:12.7Z",
            None,
        ),
        ("", "2024-03-30T23:30:12.7Z", None),
    ];

    #[test]
    fn next_matches_go() {
        let mut failed = Vec::new();
        for &(spec, t, want) in GO_CASES {
            let t: Timestamp = t.parse().expect("timestamp");
            let got = parse(spec).ok().map(|s| s.next_unix(t.as_nanosecond()));
            if got != want {
                failed.push(format!("{spec:?} after {t}: got {got:?}, go {want:?}"));
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    #[test]
    fn local_time_zone_name() {
        assert!(matches!(
            parse("CRON_TZ=Local 0 0 * * *"),
            Ok(Schedule::Spec(_))
        ));
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("1h2m3.5s"), Ok(3_723_500_000_000));
        assert_eq!(parse_duration("-1.5h"), Ok(-5_400_000_000_000));
        assert_eq!(parse_duration("+0"), Ok(0));
        assert_eq!(parse_duration("-2562047h47m16.854775808s"), Ok(i64::MIN));
        assert!(parse_duration("1").is_err());
        assert!(parse_duration(".s").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn ksm_parse_schedule() {
        assert!(parse_schedule("0 */6 * * *", Some("Invalid/Zone")).is_err());
        assert!(parse_schedule("0 */6 * * *", Some("Asia/Singapore")).is_ok());
        assert!(parse_schedule("*/0 * * * *", None).is_err());
    }
}
