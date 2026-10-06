// Spike increment 3: the §3.4 thread model.
//
// A worker thread produces frames and can only reach the QObject through
// `CxxQtThread::queue`, which runs the closure on the Qt event loop while
// holding the object lock. Everything measured here is what the real capture
// thread will cost: handoff latency, the per-frame payload copy (R2's
// "one full copy per frame" fallback) and whether the Qt event loop survives
// the load (QML counts its own timer ticks and reports the deficit).

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(i32, last_seq)]
        #[qproperty(i64, last_queue_us)]
        #[qproperty(i64, last_copy_us)]
        #[qproperty(bool, suite_running)]
        #[qproperty(QString, suite_report)]
        type FramePump = super::FramePumpRust;

        #[qinvokable]
        #[cxx_name = "runSuite"]
        fn run_suite(self: Pin<&mut Self>, quick: bool);

        #[qinvokable]
        #[cxx_name = "stopSuite"]
        fn stop_suite(self: Pin<&mut Self>);

        #[qsignal]
        #[cxx_name = "configStarted"]
        fn config_started(self: Pin<&mut Self>, idx: i32, name: &QString, hz: i32, frames: i32, width: i32, height: i32, copy: bool);

        #[qsignal]
        #[cxx_name = "configFinished"]
        fn config_finished(
            self: Pin<&mut Self>,
            idx: i32,
            name: &QString,
            delivered: i32,
            hz_achieved: i32,
            avg_queue_us: i64,
            worst_queue_us: i64,
            avg_copy_us: i64,
            worst_copy_us: i64,
            wall_ms: i64,
        );

        #[qsignal]
        #[cxx_name = "suiteFinished"]
        fn suite_finished(self: Pin<&mut Self>, report: &QString);
    }

    // Threading is opt-in per QObject. Without this line `qt_thread()` does not exist.
    impl cxx_qt::Threading for FramePump {}
}

use core::pin::Pin;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct FramePumpRust {
    last_seq: i32,
    last_queue_us: i64,
    last_copy_us: i64,
    suite_running: bool,
    suite_report: QString,
    stop: Arc<AtomicBool>,
}

struct Config {
    name: &'static str,
    hz: i32,
    frames: i32,
    w: i32,
    h: i32,
    /// Copy the payload into a buffer on the Qt thread, i.e. R2's full-copy fallback.
    copy: bool,
    /// 0 = never drop (what a naive capture thread does). >0 = coalesce: skip a
    /// frame when this many are already posted and not yet landed.
    max_inflight: i32,
    /// Simulated per-frame work on the Qt thread (stands in for texture upload +
    /// scene re-render, which the memcpy alone under-represents).
    spin_us: i32,
}

const CONFIGS: &[Config] = &[
    Config { name: "queue-only 64B@60Hz", hz: 60, frames: 150, w: 1, h: 1, copy: false, max_inflight: 0, spin_us: 0 },
    Config { name: "1080p RGBA@60Hz", hz: 60, frames: 150, w: 1920, h: 1080, copy: true, max_inflight: 0, spin_us: 0 },
    Config { name: "1080p RGBA@120Hz (overproduce)", hz: 120, frames: 240, w: 1920, h: 1080, copy: true, max_inflight: 0, spin_us: 0 },
    Config { name: "4K RGBA@30Hz", hz: 30, frames: 60, w: 3840, h: 2160, copy: true, max_inflight: 0, spin_us: 0 },
    Config { name: "1080p RGBA@240Hz coalesce<=2", hz: 240, frames: 240, w: 1920, h: 1080, copy: true, max_inflight: 2, spin_us: 0 },
    // Saturated Qt thread: 12 ms of per-frame work arriving every 8.3 ms.
    Config { name: "1080p@120Hz work=12ms UNCAPPED", hz: 120, frames: 240, w: 1920, h: 1080, copy: true, max_inflight: 0, spin_us: 12000 },
    Config { name: "1080p@120Hz work=12ms coalesce<=2", hz: 120, frames: 240, w: 1920, h: 1080, copy: true, max_inflight: 2, spin_us: 12000 },
];

#[derive(Default)]
struct Stats {
    landed: AtomicI64,
    sum_queue_us: AtomicI64,
    worst_queue_us: AtomicI64,
    sum_copy_us: AtomicI64,
    worst_copy_us: AtomicI64,
}

impl Stats {
    fn record(&self, queue_us: i64, copy_us: i64) {
        self.landed.fetch_add(1, Ordering::Relaxed);
        self.sum_queue_us.fetch_add(queue_us, Ordering::Relaxed);
        self.sum_copy_us.fetch_add(copy_us, Ordering::Relaxed);
        self.worst_queue_us.fetch_max(queue_us, Ordering::Relaxed);
        self.worst_copy_us.fetch_max(copy_us, Ordering::Relaxed);
    }
}

thread_local! {
    /// Stands in for the texture/backing store the frame lands in. Reused, so the
    /// measured cost is memcpy and not allocation.
    static DST: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Windows `sleep` has ~15.6 ms granularity by default, so sleeping to a 16 ms
/// deadline overshoots. Sleep the bulk, then spin the last few ms; the reported
/// achieved-Hz is measured, not assumed.
fn wait_until(target: Instant) {
    while let Some(rem) = target.checked_duration_since(Instant::now()) {
        if rem > Duration::from_millis(6) {
            std::thread::sleep(rem - Duration::from_millis(3));
        } else {
            std::hint::spin_loop();
        }
    }
}

/// Stands in for the Qt-thread work a real frame triggers beyond the copy:
/// texture upload + scene re-render. Busy-wait, so the cost is deterministic
/// instead of machine-dependent.
fn burn(us: i32) {
    if us <= 0 {
        return;
    }
    let until = Instant::now() + Duration::from_micros(us as u64);
    while Instant::now() < until {
        std::hint::spin_loop();
    }
}

impl qobject::FramePump {
    pub fn run_suite(self: Pin<&mut Self>, quick: bool) {
        let qt_thread = self.qt_thread();
        let stop = self.rust().stop.clone();
        stop.store(false, Ordering::SeqCst);

        std::thread::spawn(move || {
            let mut lines: Vec<String> = Vec::new();
            let suite_start = Instant::now();

            for (idx, cfg) in CONFIGS.iter().enumerate() {
                if stop.load(Ordering::SeqCst) || qt_thread.is_destroyed() {
                    break;
                }
                let frames = if quick { (cfg.frames / 5).max(10) } else { cfg.frames };
                let name = cfg.name;
                let stats = Arc::new(Stats::default());

                // Event-loop ordering is FIFO, so this lands before the frames.
                let _ = qt_thread.queue(move |mut obj| {
                    let q = QString::from(name);
                    obj.as_mut().announce(idx as i32, &q, cfg.hz, frames, cfg.w, cfg.h, cfg.copy);
                });

                let config_start = Instant::now();
                let period = Duration::from_nanos(1_000_000_000 / cfg.hz as u64);
                let len = (cfg.w as usize) * (cfg.h as usize) * 4;
                let mut queued = 0i64;

                let mut dropped = 0i64;
                for seq in 0..frames {
                    if stop.load(Ordering::SeqCst) || qt_thread.is_destroyed() {
                        break;
                    }
                    wait_until(config_start + period * (seq as u32 + 1));

                    // Coalesce policy: never let more than max_inflight frames pile onto
                    // the event loop. A screenshot mask must show the *newest* frame, so
                    // dropping is correct and queueing everything is not.
                    if cfg.max_inflight > 0
                        && queued - stats.landed.load(Ordering::Relaxed) >= cfg.max_inflight as i64
                    {
                        dropped += 1;
                        continue;
                    }

                    let payload = if cfg.copy { vec![0xAAu8; len] } else { vec![0u8; 64] };
                    let stats = stats.clone();
                    let copy = cfg.copy;
                    let spin_us = cfg.spin_us;
                    let sent = Instant::now();
                    queued += 1;
                    // queue() never blocks the worker; a full event loop just means
                    // the closure lands late, which is exactly what latency measures.
                    let _ = qt_thread.queue(move |mut obj| {
                        let queue_us = sent.elapsed().as_micros() as i64;
                        let mut copy_us = 0i64;
                        if copy {
                            copy_us = DST.with(|dst| {
                                let mut dst = dst.borrow_mut();
                                if dst.len() != payload.len() {
                                    dst.resize(payload.len(), 0);
                                }
                                let t = Instant::now();
                                dst.copy_from_slice(&payload);
                                t.elapsed().as_micros() as i64
                            });
                        }
                        burn(spin_us); stats.record(queue_us, copy_us);
                        obj.as_mut().note_frame(seq as i32, queue_us, copy_us);
                    });
                }

                // Drain: report what actually landed on the Qt thread, not what was posted.
                let drain_deadline = Instant::now() + Duration::from_secs(3);
                while stats.landed.load(Ordering::Relaxed) < queued
                    && !qt_thread.is_destroyed()
                    && Instant::now() < drain_deadline
                {
                    std::thread::sleep(Duration::from_millis(1));
                }

                let landed = stats.landed.load(Ordering::Relaxed);
                let n = landed.max(1);
                let avg_queue_us = stats.sum_queue_us.load(Ordering::Relaxed) / n;
                let avg_copy_us = stats.sum_copy_us.load(Ordering::Relaxed) / n;
                let wall_ms = config_start.elapsed().as_millis() as i64;
                let line = format!(
                    "[cfg {}] {:<32} landed={}/{} dropped={} hz={}  avgQueue={}us  worstQueue={}us  avgCopy={}us  worstCopy={}us  wall={}ms",
                    idx + 1,
                    name,
                    landed,
                    queued,
                    dropped,
                    (landed as f64 * 1000.0 / wall_ms.max(1) as f64) as i32,
                    avg_queue_us,
                    stats.worst_queue_us.load(Ordering::Relaxed),
                    avg_copy_us,
                    stats.worst_copy_us.load(Ordering::Relaxed),
                    wall_ms,
                );
                println!("[rust thread] {line}");
                lines.push(line);

                let report = lines.join("\n");
                let _ = qt_thread.queue(move |mut obj| {
                    let q = QString::from(name);
                    let r = QString::from(&*report);
                    obj.as_mut().conclude(
                        idx as i32,
                        &q,
                        landed as i32,
                        (landed as f64 * 1000.0 / wall_ms.max(1) as f64) as i32,
                        avg_queue_us,
                        stats.worst_queue_us.load(Ordering::Relaxed),
                        avg_copy_us,
                        stats.worst_copy_us.load(Ordering::Relaxed),
                        wall_ms,
                        &r,
                    );
                });
            }

            let total_ms = suite_start.elapsed().as_millis() as i64;
            let aborted = stop.load(Ordering::SeqCst);
            let summary = format!(
                "{} configs in {} ms{}\n{}",
                lines.len(),
                total_ms,
                if aborted { " (stopped)" } else { "" },
                lines.join("\n")
            );
            let _ = qt_thread.queue(move |mut obj| {
                let s = QString::from(&*summary);
                obj.as_mut().finish(&s);
            });
        });
    }

    pub fn stop_suite(self: Pin<&mut Self>) {
        self.rust().stop.store(true, Ordering::SeqCst);
    }

    fn announce(
        mut self: Pin<&mut Self>,
        idx: i32,
        name: &QString,
        hz: i32,
        frames: i32,
        width: i32,
        height: i32,
        copy: bool,
    ) {
        self.as_mut().set_suite_running(true);
        self.config_started(idx, name, hz, frames, width, height, copy);
    }

    /// Per-frame work on the Qt thread. `lastSeq` / `lastQueueUs` / `lastCopyUs`
    /// are bound to QML Labels, so these writes carry the binding re-evaluation
    /// cost measured in §3.2 rule 4 — that is the realistic path, not a tax we
    /// can ignore.
    fn note_frame(mut self: Pin<&mut Self>, seq: i32, queue_us: i64, copy_us: i64) {
        self.as_mut().set_last_seq(seq);
        self.as_mut().set_last_queue_us(queue_us);
        self.as_mut().set_last_copy_us(copy_us);
    }

    #[allow(clippy::too_many_arguments)]
    fn conclude(
        mut self: Pin<&mut Self>,
        idx: i32,
        name: &QString,
        delivered: i32,
        hz_achieved: i32,
        avg_queue_us: i64,
        worst_queue_us: i64,
        avg_copy_us: i64,
        worst_copy_us: i64,
        wall_ms: i64,
        report: &QString,
    ) {
        self.as_mut().set_suite_report(QString::from(report.to_string().as_str()));
        self.config_finished(
            idx,
            name,
            delivered,
            hz_achieved,
            avg_queue_us,
            worst_queue_us,
            avg_copy_us,
            worst_copy_us,
            wall_ms,
        );
    }

    fn finish(mut self: Pin<&mut Self>, report: &QString) {
        self.as_mut().set_suite_running(false);
        self.as_mut().set_suite_report(QString::from(report.to_string().as_str()));
        self.suite_finished(report);
    }
}
