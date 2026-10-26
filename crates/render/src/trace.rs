//! Local profiling aid (not for upstream), on unless `DESIGNCRAFT_TRACE=0`: times what the app does and
//! writes it to the log (`designcraft.log`, target `designcraft_render::trace`).
//!
//! - Spans: `let _s = trace::span(20.0, || format!("…"));` logs the span when it takes longer
//!   than the threshold (ms), with the spans it ran inside of.
//! - Stalls: a watchdog thread looks at every thread's open spans ten times a second. A thread
//!   inside the same outermost span for more than [`STALL_MS`] is logged as a stall with what it
//!   is doing right now, again every second while it lasts.
//! - Load: once a second, when anything was slow or stalled, the CPU time of the process and of
//!   its busiest threads (from `/proc`), so a hang with idle cores shows as such.
//!
//! Off (the default) every call is one atomic load.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// A thread inside one outermost span this long is stalled.
pub const STALL_MS: u128 = 400;

static ON: AtomicBool = AtomicBool::new(false);
/// Something was slow since the last load report (makes the next report worth logging).
static SLOW: AtomicBool = AtomicBool::new(false);
/// The longest outermost span since the last load report, in microseconds.
static WORST_US: AtomicU64 = AtomicU64::new(0);

struct Open {
    label: String,
    start: Instant,
}

struct Stack {
    thread: String,
    open: Vec<Open>,
    /// The outermost span's start a stall was last logged for, and when.
    reported: Option<(Instant, Instant)>,
}

fn stacks() -> &'static Mutex<Vec<Arc<Mutex<Stack>>>> {
    static STACKS: OnceLock<Mutex<Vec<Arc<Mutex<Stack>>>>> = OnceLock::new();
    STACKS.get_or_init(|| Mutex::new(Vec::new()))
}

thread_local! {
    static MINE: RefCell<Option<Arc<Mutex<Stack>>>> = const { RefCell::new(None) };
}

fn mine() -> Arc<Mutex<Stack>> {
    MINE.with(|m| {
        m.borrow_mut()
            .get_or_insert_with(|| {
                let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
                let s = Arc::new(Mutex::new(Stack { thread, open: Vec::new(), reported: None }));
                stacks().lock().unwrap_or_else(PoisonError::into_inner).push(s.clone());
                s
            })
            .clone()
    })
}

/// Is tracing on?
#[inline]
pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Turn tracing on when `DESIGNCRAFT_TRACE` is set (and not `0`), and start the watchdog.
pub fn init() {
    // This local build traces unless told not to (`DESIGNCRAFT_TRACE=0`).
    let on = std::env::var("DESIGNCRAFT_TRACE").map_or(true, |v| v != "0");
    if !on || ON.swap(true, Ordering::Relaxed) {
        return;
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    log::info!("trace on: spans over their threshold, stalls over {STALL_MS} ms, CPU load when anything was slow ({cores} cores)");
    let _ = std::thread::Builder::new().name("designcraft-trace".into()).spawn(move || watchdog(cores));
}

/// An open span; logs itself on drop when it took longer than its threshold.
pub struct Span {
    threshold_ms: f64,
    start: Instant,
    depth: usize,
    stack: Arc<Mutex<Stack>>,
}

/// Open a span labelled `label()` (only built when tracing is on).
pub fn span(threshold_ms: f64, label: impl FnOnce() -> String) -> Option<Span> {
    if !enabled() {
        return None;
    }
    let stack = mine();
    let start = Instant::now();
    let depth = {
        let mut s = stack.lock().unwrap_or_else(PoisonError::into_inner);
        s.open.push(Open { label: label(), start });
        s.open.len()
    };
    Some(Span { threshold_ms, start, depth, stack })
}

impl Drop for Span {
    fn drop(&mut self) {
        let took = self.start.elapsed();
        let ms = took.as_secs_f64() * 1000.0;
        let mut s = self.stack.lock().unwrap_or_else(PoisonError::into_inner);
        if self.depth == 1 {
            WORST_US.fetch_max(took.as_micros() as u64, Ordering::Relaxed);
        }
        if ms >= self.threshold_ms {
            SLOW.store(true, Ordering::Relaxed);
            let path: Vec<&str> = s.open.iter().take(self.depth).map(|o| o.label.as_str()).collect();
            log::info!("[{}] {ms:.1} ms  {}", s.thread, path.join(" > "));
        }
        s.open.truncate(self.depth.saturating_sub(1));
    }
}

/// CPU time (clock ticks) of the process and of each thread, with the threads' names.
fn cpu_ticks() -> (u64, Vec<(u64, String, u64)>) {
    // utime and stime are fields 14 and 15; the name in parentheses may hold spaces.
    let ticks = |stat: &str| -> Option<u64> {
        let rest = &stat[stat.rfind(')')? + 2..];
        let f: Vec<&str> = rest.split_whitespace().collect();
        Some(f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?)
    };
    let total = std::fs::read_to_string("/proc/self/stat").ok().and_then(|s| ticks(&s)).unwrap_or(0);
    let mut threads = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/proc/self/task") {
        for e in dir.flatten() {
            let tid = e.file_name().to_string_lossy().parse::<u64>().unwrap_or(0);
            let p = e.path();
            let name = std::fs::read_to_string(p.join("comm")).unwrap_or_default().trim().to_string();
            if let Some(t) = std::fs::read_to_string(p.join("stat")).ok().and_then(|s| ticks(&s)) {
                threads.push((tid, name, t));
            }
        }
    }
    (total, threads)
}

fn watchdog(cores: usize) {
    // Linux reports CPU time in 1/100 s.
    const HZ: f64 = 100.0;
    let mut last_load = Instant::now();
    let mut last = cpu_ticks();
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let now = Instant::now();
        let all: Vec<_> = stacks().lock().unwrap_or_else(PoisonError::into_inner).clone();
        let mut stalled = false;
        for st in all {
            let mut s = st.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(outer) = s.open.first().map(|o| o.start) else { continue };
            let held = now.duration_since(outer).as_millis();
            if held < STALL_MS {
                continue;
            }
            stalled = true;
            let due = match s.reported {
                Some((o, at)) if o == outer => now.duration_since(at) >= Duration::from_secs(1),
                _ => true,
            };
            if due {
                let path: Vec<String> = s.open.iter().map(|o| format!("{} ({} ms)", o.label, now.duration_since(o.start).as_millis())).collect();
                log::warn!("STALL [{}] {held} ms so far: {}", s.thread, path.join(" > "));
                s.reported = Some((outer, now));
            }
        }
        if stalled {
            SLOW.store(true, Ordering::Relaxed);
        }
        let secs = now.duration_since(last_load).as_secs_f64();
        if secs < 1.0 {
            continue;
        }
        let cur = cpu_ticks();
        if SLOW.swap(false, Ordering::Relaxed) {
            let busy = cur.0.saturating_sub(last.0) as f64 / HZ / secs;
            let mut per: Vec<(f64, String)> = cur
                .1
                .iter()
                .map(|(tid, name, t)| {
                    let before = last.1.iter().find(|(id, _, _)| id == tid).map_or(0, |x| x.2);
                    (t.saturating_sub(before) as f64 / HZ / secs, name.clone())
                })
                .filter(|(c, _)| *c >= 0.05)
                .collect();
            per.sort_by(|a, b| b.0.total_cmp(&a.0));
            let top: Vec<String> = per.iter().take(6).map(|(c, n)| format!("{n} {:.0}%", c * 100.0)).collect();
            let worst = WORST_US.swap(0, Ordering::Relaxed) as f64 / 1000.0;
            log::info!(
                "load: {:.2} of {cores} cores busy ({:.0}% of the machine); longest span {worst:.0} ms; threads: {}",
                busy,
                busy / cores as f64 * 100.0,
                if top.is_empty() { "all idle".to_string() } else { top.join(", ") }
            );
        } else {
            WORST_US.store(0, Ordering::Relaxed);
        }
        last = cur;
        last_load = now;
    }
}
