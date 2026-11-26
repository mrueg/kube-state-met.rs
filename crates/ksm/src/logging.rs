//! klog-style log lines on stderr (`I1126 22:59:37.336019  318666 file.go:164] "msg" key="value"`).
//! Log lines are not part of the equivalence bar (ADR-003); the layout is kept
//! so that log tooling written for kube-state-metrics keeps working.

use std::fmt::Write as _;

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

fn header(level: char, file: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let (mon, day, h, m, s) = civil(secs);
    format!(
        "{level}{mon:02}{day:02} {h:02}:{m:02}:{s:02}.{:06} {:>7} {file}] ",
        now.subsec_micros(),
        std::process::id()
    )
}

/// Month, day, hour, minute, second (UTC) of a Unix time.
fn civil(secs: i64) -> (i64, i64, i64, i64, i64) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let doe = z - z.div_euclid(146_097) * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    (mo, d, rem / 3600, (rem % 3600) / 60, rem % 60)
}

fn quote(s: &str) -> String {
    format!("{s:?}")
}

/// An error line as Go's `klog.ErrorS(err, msg)` at `file` prints it.
pub fn error_go(file: &str, msg: &str, err: &str) {
    eprintln!("{}{} err={}", header('E', file), quote(msg), quote(err));
}

#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            let v = format!("{value:?}");
            let _ = write!(self.rest, " {}={}", field.name(), quote(v.trim_matches('"')));
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            let _ = write!(self.rest, " {}={}", field.name(), quote(value));
        }
    }
}

struct Klog;

impl<S: Subscriber> Layer<S> for Klog {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = match *event.metadata().level() {
            Level::ERROR => 'E',
            Level::WARN => 'W',
            _ => 'I',
        };
        let mut f = Fields::default();
        event.record(&mut f);
        let file = event
            .metadata()
            .file()
            .and_then(|p| p.rsplit('/').next())
            .unwrap_or("main.rs");
        let line = event.metadata().line().unwrap_or(0);
        eprintln!("{}{}{}", header(level, &format!("{file}:{line}")), quote(&f.message), f.rest);
    }
}

/// Installs the logger; `-v` above 0 enables debug, above 4 trace.
pub fn init(verbosity: i32) {
    let max = match verbosity {
        v if v >= 5 => tracing::level_filters::LevelFilter::TRACE,
        v if v >= 1 => tracing::level_filters::LevelFilter::DEBUG,
        _ => tracing::level_filters::LevelFilter::INFO,
    };
    let filter = tracing_subscriber::filter::Targets::new()
        .with_default(max)
        .with_target("kube_client", tracing::level_filters::LevelFilter::WARN)
        .with_target("kube_runtime", tracing::level_filters::LevelFilter::WARN)
        .with_target("hyper", tracing::level_filters::LevelFilter::WARN)
        .with_target("rustls", tracing::level_filters::LevelFilter::WARN);
    let _ = tracing_subscriber::registry().with(Klog.with_filter(filter)).try_init();
}
