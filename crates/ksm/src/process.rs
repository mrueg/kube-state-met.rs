//! `process_*` metrics of the telemetry port, read from procfs (Go:
//! client_golang's process collector). Empty on systems without procfs.

/// One family: name, help, type and samples.
pub type ExtraFamily = (
    String,
    String,
    &'static str,
    Vec<(Vec<(String, String)>, f64)>,
);

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Clock ticks per second; Linux exposes USER_HZ = 100 to user space.
const USER_HZ: f64 = 100.0;

/// The page size (`os.Getpagesize`), from the aux vector's `AT_PAGESZ`:
/// 4 KiB on most machines, 16 or 64 KiB on some arm64 kernels. Read once.
fn page_size() -> f64 {
    static PAGE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *PAGE.get_or_init(read_page_size)
}

fn read_page_size() -> f64 {
    const AT_PAGESZ: usize = 6;
    const W: usize = std::mem::size_of::<usize>();
    let word = |b: &[u8]| usize::from_ne_bytes(b.try_into().unwrap_or([0; W]));
    std::fs::read("/proc/self/auxv")
        .ok()
        .and_then(|auxv| {
            auxv.as_chunks::<{ 2 * W }>()
                .0
                .iter()
                .find(|e| word(&e[..W]) == AT_PAGESZ)
                .map(|e| word(&e[W..]) as f64)
        })
        .filter(|p| *p > 0.0)
        .unwrap_or(4096.0)
}

fn family(name: &str, help: &str, ty: &'static str, v: f64) -> ExtraFamily {
    (name.to_owned(), help.to_owned(), ty, vec![(Vec::new(), v)])
}

/// Every process family that could be read.
pub fn families() -> Vec<ExtraFamily> {
    let mut out = Vec::new();
    let Some(stat) = read("/proc/self/stat") else {
        return out;
    };
    // Fields after the parenthesised command name.
    let rest = stat.rsplit_once(')').map_or("", |(_, r)| r);
    let f: Vec<&str> = rest.split_whitespace().collect();
    let num = |i: usize| f.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    // Indexes are field numbers from proc(5) minus 3.
    let utime = num(11);
    let stime = num(12);
    let start_ticks = num(19);
    let vsize = num(20);
    let rss = num(21);
    out.push(family(
        "process_cpu_seconds_total",
        "Total user and system CPU time spent in seconds.",
        "counter",
        (utime + stime) / USER_HZ,
    ));
    if let Some(limits) = read("/proc/self/limits") {
        let limit = |prefix: &str| {
            limits
                .lines()
                .find(|l| l.starts_with(prefix))
                .and_then(|l| {
                    let soft = l[prefix.len()..].split_whitespace().next()?;
                    Some(if soft == "unlimited" {
                        f64::INFINITY
                    } else {
                        soft.parse().ok()?
                    })
                })
        };
        if let Some(v) = limit("Max open files") {
            out.push(family(
                "process_max_fds",
                "Maximum number of open file descriptors.",
                "gauge",
                v,
            ));
        }
        if let Some(v) = limit("Max address space") {
            out.push(family(
                "process_virtual_memory_max_bytes",
                "Maximum amount of virtual memory available in bytes.",
                "gauge",
                v,
            ));
        }
    }
    if let Some(net) = read("/proc/self/net/dev") {
        let (mut rx, mut tx) = (0.0, 0.0);
        for line in net.lines().skip(2) {
            if let Some((_, data)) = line.split_once(':') {
                let v: Vec<f64> = data
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                rx += v.first().copied().unwrap_or(0.0);
                tx += v.get(8).copied().unwrap_or(0.0);
            }
        }
        out.push(family(
            "process_network_receive_bytes_total",
            "Number of bytes received by the process over the network.",
            "counter",
            rx,
        ));
        out.push(family(
            "process_network_transmit_bytes_total",
            "Number of bytes sent by the process over the network.",
            "counter",
            tx,
        ));
    }
    if let Ok(fds) = std::fs::read_dir("/proc/self/fd") {
        out.push(family(
            "process_open_fds",
            "Number of open file descriptors.",
            "gauge",
            fds.count() as f64,
        ));
    }
    out.push(family(
        "process_resident_memory_bytes",
        "Resident memory size in bytes.",
        "gauge",
        rss * page_size(),
    ));
    if let Some(boot) = read("/proc/stat").and_then(|s| {
        s.lines().find_map(|l| {
            l.strip_prefix("btime ")
                .and_then(|v| v.trim().parse::<f64>().ok())
        })
    }) {
        out.push(family(
            "process_start_time_seconds",
            "Start time of the process since unix epoch in seconds.",
            "gauge",
            boot + start_ticks / USER_HZ,
        ));
    }
    out.push(family(
        "process_virtual_memory_bytes",
        "Virtual memory size in bytes.",
        "gauge",
        vsize,
    ));
    out
}
