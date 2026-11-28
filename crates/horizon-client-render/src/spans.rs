//! Named timings within a frame, for finding what makes a frame slow:
//! `HORIZON_SPIKES=MS` prints the spans of every frame whose CPU time
//! exceeds MS milliseconds, and of every server tick (dedicated or
//! integrated) longer than MS. Each span keeps its wall time and the
//! thread's CPU time inside it, so a span that waited (for a lock, the GPU
//! or a core on a busy machine) shows apart from one that worked. Costs two
//! clock reads per span when enabled and a relaxed load otherwise.
use std::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

static ENABLED: AtomicBool = AtomicBool::new(false);
thread_local! {
    static SPANS: RefCell<Vec<Span>> = const { RefCell::new(Vec::new()) };
    static REPORT: RefCell<Report> = RefCell::new(Report::default());
}

/// A named timing: wall milliseconds and the thread's CPU milliseconds in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub name: &'static str,
    pub ms: f32,
    pub cpu_ms: f32,
}

pub fn enable(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}
/// Run `f`, recording its duration under `name` when spans are enabled.
#[inline]
pub fn time<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    if !ENABLED.load(Ordering::Relaxed) {
        return f();
    }
    let (started, cpu) = (Instant::now(), thread_cpu());
    let out = f();
    let ms = started.elapsed().as_secs_f32() * 1000.0;
    let cpu_ms = thread_cpu()
        .zip(cpu)
        .map_or(ms, |(end, start)| (end - start).as_secs_f32() * 1000.0);
    SPANS.with(|s| s.borrow_mut().push(Span { name, ms, cpu_ms }));
    out
}
/// Records the wall time since `started` under `name`, for code a closure
/// cannot wrap (its CPU time is taken as the wall time).
pub fn record(name: &'static str, started: Instant) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let ms = started.elapsed().as_secs_f32() * 1000.0;
    SPANS.with(|s| s.borrow_mut().push(Span { name, ms, cpu_ms: ms }));
}
/// The CPU time this thread has used, where the platform tells it: a tick
/// with more wall time than CPU time waited (for a lock, or for a core on a
/// busy machine).
pub fn thread_cpu() -> Option<Duration> {
    #[cfg(unix)]
    {
        let t = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
        Some(Duration::new(t.tv_sec as u64, t.tv_nsec as u32))
    }
    #[cfg(not(unix))]
    horizon_threads::thread_cpu()
}

/// The frame's spans, summed by name, slowest first; clears them.
pub fn take() -> Vec<Span> {
    let spans = SPANS.with(|s| std::mem::take(&mut *s.borrow_mut()));
    let mut sums: Vec<Span> = Vec::new();
    for span in spans {
        match sums.iter_mut().find(|s| s.name == span.name) {
            Some(sum) => {
                sum.ms += span.ms;
                sum.cpu_ms += span.cpu_ms;
            }
            None => sums.push(span),
        }
    }
    sums.sort_by(|a, b| b.ms.total_cmp(&a.ms));
    sums
}
/// `name wall` for spans of at least 0.5 ms, with `/cpu` when the thread
/// worked for less than nine tenths of it: the slow-frame and slow-tick lines.
pub fn describe(spans: &[Span]) -> String {
    let list: Vec<String> = spans
        .iter()
        .filter(|s| s.ms >= 0.5)
        .map(|s| {
            if s.cpu_ms < s.ms * 0.9 {
                format!("{} {:.1}/{:.1}", s.name, s.ms, s.cpu_ms)
            } else {
                format!("{} {:.1}", s.name, s.ms)
            }
        })
        .collect();
    list.join(", ")
}

/// Spans summed over many frames (`HORIZON_SPAN_REPORT=FRAMES`), for a
/// steady cost that never crosses the spike threshold. A tick runs in only
/// some frames, so each name keeps its total, its slowest frame and how many
/// frames had it.
#[derive(Default)]
pub struct Report {
    frames: u32,
    rows: Vec<ReportRow>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ReportRow {
    name: &'static str,
    total_ms: f64,
    max_ms: f32,
    frames: u32,
}

impl Report {
    /// Adds one frame's spans (from [`take`], summed by name).
    pub fn add(&mut self, spans: &[Span]) {
        self.frames += 1;
        for span in spans {
            match self.rows.iter_mut().find(|r| r.name == span.name) {
                Some(row) => {
                    row.total_ms += f64::from(span.ms);
                    row.max_ms = row.max_ms.max(span.ms);
                    row.frames += 1;
                }
                None => self.rows.push(ReportRow {
                    name: span.name,
                    total_ms: f64::from(span.ms),
                    max_ms: span.ms,
                    frames: 1,
                }),
            }
        }
    }
    pub fn frames(&self) -> u32 {
        self.frames
    }
    /// `frames=N` and, per name, costliest first, `name per_frame/per_run/max/nK`:
    /// milliseconds averaged over every frame, over the K frames that had
    /// it, and its slowest frame. Starts over.
    pub fn take_line(&mut self) -> String {
        let counted = std::mem::take(&mut self.frames);
        let frames = counted.max(1);
        let mut rows = std::mem::take(&mut self.rows);
        rows.sort_by(|a, b| b.total_ms.total_cmp(&a.total_ms));
        let list: Vec<String> = rows
            .iter()
            .map(|r| {
                format!(
                    "{} {:.2}/{:.2}/{:.1}/n{}",
                    r.name,
                    r.total_ms / f64::from(frames),
                    r.total_ms / f64::from(r.frames.max(1)),
                    r.max_ms,
                    r.frames
                )
            })
            .collect();
        format!("frames={counted} {}", list.join(", "))
    }
}

/// Adds a frame's spans to this thread's report; once it holds `every`
/// frames, returns its line and starts over.
pub fn report_frame(spans: &[Span], every: u32) -> Option<String> {
    REPORT.with(|r| {
        let mut report = r.borrow_mut();
        report.add(spans);
        (report.frames() >= every.max(1)).then(|| report.take_line())
    })
}

#[cfg(test)]
mod tests {
    use super::Span;

    /// Each name's cost averages over every frame and over the frames that
    /// ran it: a tick in one frame of four shows both.
    #[test]
    fn report_averages_over_frames_and_runs() {
        let tick = |ms| Span {
            name: "tick",
            ms,
            cpu_ms: ms,
        };
        let hud = Span {
            name: "hud",
            ms: 0.5,
            cpu_ms: 0.5,
        };
        let mut report = super::Report::default();
        report.add(&[tick(8.0), hud]);
        report.add(&[hud]);
        report.add(&[hud]);
        report.add(&[tick(4.0), hud]);
        assert_eq!(report.frames(), 4);
        assert_eq!(
            report.take_line(),
            "frames=4 tick 3.00/6.00/8.0/n2, hud 0.50/0.50/0.5/n4"
        );
        assert_eq!(report.frames(), 0);
        assert_eq!(report.take_line(), "frames=0 ");
    }

    #[test]
    fn report_frame_returns_a_line_every_n_frames() {
        let span = Span {
            name: "a",
            ms: 1.0,
            cpu_ms: 1.0,
        };
        assert_eq!(super::report_frame(&[span], 2), None);
        assert_eq!(
            super::report_frame(&[span], 2).as_deref(),
            Some("frames=2 a 1.00/1.00/1.0/n2")
        );
        assert_eq!(super::report_frame(&[span], 2), None);
    }

    /// Spans of one name add up; a span the thread spent mostly waiting
    /// shows its CPU time after the wall time, short ones are left out.
    #[test]
    fn spans_sum_by_name_and_show_waits() {
        super::SPANS.with(|s| {
            s.borrow_mut().extend([
                Span {
                    name: "tick",
                    ms: 2.0,
                    cpu_ms: 2.0,
                },
                Span {
                    name: "submit",
                    ms: 30.0,
                    cpu_ms: 0.2,
                },
                Span {
                    name: "tick",
                    ms: 3.0,
                    cpu_ms: 2.9,
                },
                Span {
                    name: "hud",
                    ms: 0.3,
                    cpu_ms: 0.3,
                },
            ]);
        });
        let spans = super::take();
        assert_eq!(
            spans[0],
            Span {
                name: "submit",
                ms: 30.0,
                cpu_ms: 0.2
            }
        );
        assert_eq!((spans[1].name, spans[1].ms), ("tick", 5.0));
        assert_eq!(super::describe(&spans), "submit 30.0/0.2, tick 5.0");
        assert!(super::take().is_empty());
    }

    /// The thread's CPU clock moves with work done on it, not with time
    /// spent asleep.
    #[cfg(any(unix, all(windows, target_arch = "x86_64")))]
    #[test]
    fn thread_cpu_counts_work_not_sleep() {
        let start = super::thread_cpu().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let slept = super::thread_cpu().unwrap() - start;
        // Busy until the clock shows 10 ms of work (a loaded machine gives
        // the thread less than its wall time).
        let mut x = 0u64;
        let busy = std::time::Instant::now();
        let worked = loop {
            x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(7));
            let worked = super::thread_cpu().unwrap() - start - slept;
            if worked.as_millis() >= 10 || busy.elapsed().as_secs() >= 10 {
                break worked;
            }
        };
        assert!(slept.as_millis() < 25, "{slept:?} while asleep");
        assert!(worked.as_millis() >= 10, "{worked:?} while busy");
    }
}
