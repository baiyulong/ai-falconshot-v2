// Spike increment 4: the P1 mask-latency probe.
//
// Question the plan asks: with the frozen desktop as a QML texture, plus a dim
// layer, a hole and a live-dragged selection, does hotkey->mask-visible stay under
// 150 ms and the drag frame time under 8 ms?
//
// Rust owns the state machine and every timestamp; QML only binds properties and
// forwards frameSwapped. The 16 ms tick() is both session pacer and watchdog, so
// the numbers are reproducible without a human touching the mouse.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    // The P1 shim: Win32 capture, the QQuickImageProvider subclass and the engine
    // pointer are all C++-only. In production this block lives in crates/qt_bridge.
    unsafe extern "C++" {
        include!("hello-cxxqt/frozen_source.h");

        /// BitBlt the virtual desktop into the shared frame, microseconds.
        #[rust_name = "capture"]
        fn frozenCapture() -> i64;

        /// Fabricate a frame of an arbitrary size, microseconds.
        #[rust_name = "synthesize"]
        fn frozenSynthesize(width: i32, height: i32) -> i64;

        #[rust_name = "frame_width"]
        fn frozenWidth() -> i32;
        #[rust_name = "frame_height"]
        fn frozenHeight() -> i32;

        /// Mean luma inside vs outside the hole rect of the frame currently captured.
        #[rust_name = "self_check"]
        fn frozenSelfCheck(x0: i32, y0: i32, x1: i32, y1: i32) -> QString;

        /// The same, on what the scene graph rendered rather than on a GDI BitBlt.
        #[rust_name = "window_check"]
        fn frozenWindowCheck(x0: i32, y0: i32, x1: i32, y1: i32) -> QString;
        #[rust_name = "provider_installed"]
        fn frozenProviderInstalled() -> bool;
        #[rust_name = "provider_calls"]
        fn frozenProviderCalls() -> i64;
        #[rust_name = "provider_avg_us"]
        fn frozenProviderAvgUs() -> i64;
        #[rust_name = "provider_worst_us"]
        fn frozenProviderWorstUs() -> i64;
        #[rust_name = "describe"]
        fn frozenDescribe() -> QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        /// 0 = idle, 1 = waiting for the first frame, 2 = dragging
        #[qproperty(i32, state)]
        /// Bumped per session so the QML Image asks the provider again.
        #[qproperty(i32, token)]
        #[qproperty(bool, mask_visible)]
        #[qproperty(bool, running)]
        #[qproperty(f64, hole_x)]
        #[qproperty(f64, hole_y)]
        #[qproperty(f64, hole_w)]
        #[qproperty(f64, hole_h)]
        #[qproperty(i32, sessions_done)]
        #[qproperty(i32, sessions_planned)]
        /// P1 variant switch: false = four dim rectangles, true = one ShaderEffect.
        /// Taken from the P1_DIM environment variable, so both variants run the
        /// identical state machine.
        #[qproperty(bool, shader_dim)]
        type MaskProbe = super::MaskProbeRust;

        /// QML tells us the mask viewport in device-independent pixels.
        #[qinvokable]
        #[cxx_name = "setViewport"]
        fn set_viewport(self: Pin<&mut Self>, width: f64, height: f64);

        #[qinvokable]
        #[cxx_name = "runSuite"]
        fn run_suite(self: Pin<&mut Self>, quick: bool);

        #[qinvokable]
        #[cxx_name = "tick"]
        fn tick(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "noteSwap"]
        fn note_swap(self: Pin<&mut Self>);

        /// QML forwards ShaderEffect.status: 0=Compiled 1=Uncompiled 2=Error.
        /// Without this the dim variant could silently fall back to a blank layer.
        #[qinvokable]
        #[cxx_name = "noteShaderStatus"]
        fn note_shader_status(self: Pin<&mut Self>, status: i32);

        #[qinvokable]
        #[cxx_name = "dragStep"]
        fn drag_step(self: Pin<&mut Self>);

        /// The finished table. A signal rather than a QString qproperty on purpose:
        /// a QString field makes the backing struct !Unpin, and then every field
        /// write needs unsafe Pin projection.
        #[qsignal]
        #[cxx_name = "reportReady"]
        fn report_ready(self: Pin<&mut Self>, report: &QString);
    }
}

use core::pin::Pin;
use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use std::time::{Duration, Instant};

/// Frames measured per session once the mask is up.
const DRAG_FRAMES: usize = 24;
/// No swap for this long during a drag => the scene cannot keep up.
const DRAG_STALL_MS: u64 = 250;
/// Hotkey pressed but nothing presented => record a timeout, not a sample.
const REVEAL_TIMEOUT_MS: u64 = 3000;
/// The resolution the plan's pass line is written against, which this panel is not.
const SYNTHETIC_4K: (i32, i32) = (3840, 2160);

#[derive(Clone, Copy)]
enum Kind {
    /// Real BitBlt of this machine's desktop.
    Capture,
    /// Fabricated 4K frame: same texture path at a resolution the panel cannot offer.
    Synthetic,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Capture => "bitblt-native",
            Kind::Synthetic => "synthetic-4K",
        }
    }
}

pub struct MaskProbeRust {
    state: i32,
    token: i32,
    mask_visible: bool,
    running: bool,
    hole_x: f64,
    hole_y: f64,
    hole_w: f64,
    hole_h: f64,
    sessions_done: i32,
    sessions_planned: i32,
    shader_dim: bool,
    /// -1 until QML reports a ShaderEffect status.
    shader_status: i32,

    viewport_w: f64,
    viewport_h: f64,
    plan: Vec<Kind>,
    next_start: Instant,
    t0: Instant,
    last_swap: Instant,
    capture_us: i64,
    drag_index: usize,
    /// Input events handed to the scene this session. Compared against drag_index
    /// (presents) it separates "the render loop is saturated" from "vsync is 60Hz".
    drag_steps: usize,
    /// 16 ms heartbeats that actually got through during the drag. A full count means
    /// the Qt thread had slack, so the 60Hz present cadence is the only limit.
    drag_ticks: usize,
    ticks_seen: usize,
    ticks_expected: usize,
    steps_seen: usize,
    drag_start: Instant,
    frame_start: usize,
    reveal_ms: Vec<f64>,
    reveal_kind: Vec<&'static str>,
    frame_ms: Vec<f64>,
    started: bool,
    finished: bool,
}

impl Default for MaskProbeRust {
    fn default() -> Self {
        // Instant has no Default, so this cannot be a derive.
        let now = Instant::now();
        Self {
            state: 0,
            token: 0,
            mask_visible: false,
            running: false,
            hole_x: 0.0,
            hole_y: 0.0,
            hole_w: 0.0,
            hole_h: 0.0,
            sessions_done: 0,
            sessions_planned: 0,
            shader_dim: false,
            shader_status: -1,
            viewport_w: 1280.0,
            viewport_h: 800.0,
            plan: Vec::new(),
            next_start: now,
            t0: now,
            last_swap: now,
            capture_us: 0,
            drag_index: 0,
            drag_steps: 0,
            drag_ticks: 0,
            ticks_seen: 0,
            ticks_expected: 0,
            steps_seen: 0,
            drag_start: now,
            frame_start: 0,
            reveal_ms: Vec::new(),
            reveal_kind: Vec::new(),
            frame_ms: Vec::new(),
            started: false,
            finished: false,
        }
    }
}

fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let index = (((sorted.len() - 1) as f64) * p).round() as usize;
    sorted[index]
}

impl qobject::MaskProbe {
    fn kind_label(&self) -> &'static str {
        match self.rust().plan.get(self.rust().sessions_done as usize) {
            Some(kind) => kind.label(),
            None => "?",
        }
    }

    pub fn set_viewport(mut self: Pin<&mut Self>, width: f64, height: f64) {
        let rust = self.as_mut().rust_mut().get_mut();
        if width > 0.0 {
            rust.viewport_w = width;
        }
        if height > 0.0 {
            rust.viewport_h = height;
        }
        println!("[P1] viewport = {width}x{height} DIP");
    }

    pub fn run_suite(mut self: Pin<&mut Self>, quick: bool) {
        {
            let rust = self.as_mut().rust_mut().get_mut();
            if rust.started {
                return;
            }
            rust.started = true;
            rust.shader_dim =
                std::env::var("P1_DIM").map(|v| v == "shader").unwrap_or(false);
            let (capture_runs, synthetic_runs) = if quick { (3, 2) } else { (6, 4) };
            for i in 0..capture_runs.max(synthetic_runs) {
                if i < capture_runs {
                    rust.plan.push(Kind::Capture);
                }
                if i < synthetic_runs {
                    rust.plan.push(Kind::Synthetic);
                }
            }
            rust.sessions_planned = rust.plan.len() as i32;
            rust.next_start = Instant::now() + Duration::from_millis(300);
            let planned = rust.plan.len();
            let (w, h) = (rust.viewport_w, rust.viewport_h);
            let dim = if rust.shader_dim { "shader" } else { "rects" };
            println!("[P1] suite start: {planned} sessions, viewport={w}x{h}, dim={dim}");
        }
        self.as_mut().set_running(true);
    }

    /// 16 ms heartbeat: session pacer plus watchdog.
    pub fn tick(mut self: Pin<&mut Self>) {
        if self.rust().finished {
            if self.rust().running {
                self.as_mut().set_running(false);
            }
            return;
        }
        if !self.rust().running {
            return;
        }
        // Count heartbeats that actually fired. The pacer is a QML Timer, so a
        // missing tick means the Qt thread was busy and could not even run a
        // no-op callback — starvation independent of how often we present.
        if self.rust().state == 2 {
            self.as_mut().rust_mut().get_mut().drag_ticks += 1;
        }
        match self.rust().state {
            0 => {
                if Instant::now() >= self.rust().next_start {
                    self.begin_session();
                }
            }
            1 => {
                if self.rust().t0.elapsed() > Duration::from_millis(REVEAL_TIMEOUT_MS) {
                    println!("[P1] ABORT: no frameSwapped within {REVEAL_TIMEOUT_MS}ms");
                    self.end_session(false);
                }
            }
            _ => {
                if self.rust().last_swap.elapsed() > Duration::from_millis(DRAG_STALL_MS) {
                    let frame = self.rust().drag_index;
                    println!("[P1] stall: no swap for {DRAG_STALL_MS}ms at frame {frame}");
                    self.end_session(false);
                }
            }
        }
    }

    fn begin_session(mut self: Pin<&mut Self>) {
        let kind = match self.rust().plan.get(self.rust().sessions_done as usize) {
            Some(kind) => *kind,
            None => {
                self.as_mut().rust_mut().get_mut().finished = true;
                return;
            }
        };

        // t0 is the hotkey press: everything between here and the first presented
        // frame is what the user waits for.
        let t0 = Instant::now();
        let capture_us = match kind {
            Kind::Capture => qobject::capture(),
            Kind::Synthetic => qobject::synthesize(SYNTHETIC_4K.0, SYNTHETIC_4K.1),
        };
        let (fw, fh) = (qobject::frame_width(), qobject::frame_height());

        let (token, hole_x, hole_y, hole_w, hole_h) = {
            let rust = self.as_mut().rust_mut().get_mut();
            rust.t0 = t0;
            rust.capture_us = capture_us;
            rust.drag_index = 0;
            rust.drag_steps = 0;
            rust.drag_ticks = 0;
            rust.frame_start = rust.frame_ms.len();
            rust.token += 1;
            let w = rust.viewport_w;
            let h = rust.viewport_h;
            (rust.token, w * 0.2, h * 0.2, w * 0.3, h * 0.3)
        };
        let session = self.rust().sessions_done + 1;
        self.as_mut().set_hole_x(hole_x);
        self.as_mut().set_hole_y(hole_y);
        self.as_mut().set_hole_w(hole_w);
        self.as_mut().set_hole_h(hole_h);
        self.as_mut().set_token(token);
        self.as_mut().set_mask_visible(true);
        self.as_mut().set_state(1);
        println!("[P1] session {session} kind={} frame={fw}x{fh} capture={capture_us}us", kind.label());
    }

    /// QML Window.onFrameSwapped. The first call reveals the mask; the rest are
    /// drag frames, whose interval is the frame time the pass line is about.
    pub fn note_swap(mut self: Pin<&mut Self>) {
        let now = Instant::now();
        match self.rust().state {
            1 => {
                let reveal_ms = self.rust().t0.elapsed().as_secs_f64() * 1000.0;
                let kind = self.kind_label();
                {
                    let rust = self.as_mut().rust_mut().get_mut();
                    rust.reveal_ms.push(reveal_ms);
                    rust.reveal_kind.push(kind);
                    rust.last_swap = now;
                    rust.drag_start = now;
                }
                println!("[P1]   mask presented: reveal={reveal_ms:.1}ms");
                self.as_mut().set_state(2);
            }
            2 => {
                let (elapsed_ms, counting) = {
                    let rust = self.rust();
                    (
                        rust.last_swap.elapsed().as_secs_f64() * 1000.0,
                        rust.drag_index < DRAG_FRAMES,
                    )
                };
                if !counting {
                    return;
                }
                let done = {
                    let rust = self.as_mut().rust_mut().get_mut();
                    rust.last_swap = now;
                    rust.drag_index += 1;
                    rust.frame_ms.push(elapsed_ms);
                    rust.drag_index >= DRAG_FRAMES
                };
                if done {
                    self.end_session(true);
                }
            }
            _ => {}
        }
    }

    /// QML forwards ShaderEffect.status whenever it changes. A dim layer that failed
    /// to compile would still present frames, so the number has to be in the log.
    pub fn note_shader_status(mut self: Pin<&mut Self>, status: i32) {
        self.as_mut().rust_mut().get_mut().shader_status = status;
        let label = match status {
            0 => "Compiled",
            1 => "Uncompiled",
            2 => "Error",
            _ => "?",
        };
        println!("[P1] ShaderEffect status = {status} ({label})");
    }

    /// The 8 ms drag heartbeat: move and resize the selection, which is the scene
    /// change a real mouse move produces.
    pub fn drag_step(mut self: Pin<&mut Self>) {
        if self.rust().state != 2 {
            return;
        }
        let (index, w, h) = {
            let rust = self.rust();
            (rust.drag_index, rust.viewport_w, rust.viewport_h)
        };
        let t = (index % DRAG_FRAMES) as f64 / DRAG_FRAMES as f64;
        let size = 0.2 + 0.25 * ((t * std::f64::consts::PI * 2.0).sin().abs());
        self.as_mut().rust_mut().get_mut().drag_steps += 1;
        self.as_mut().set_hole_x(w * (0.05 + 0.65 * t));
        self.as_mut().set_hole_y(h * (0.05 + 0.6 * t));
        self.as_mut().set_hole_w(w * size);
        self.as_mut().set_hole_h(h * size);
    }

    fn end_session(mut self: Pin<&mut Self>, completed: bool) {
        let (index, kind, capture_us, frame_start, steps, ticks, expected_ticks) = {
            let rust = self.rust();
            // Only meaningful once the mask is up: before that drag_start is stale.
            let drag_wall = if rust.state == 2 {
                rust.drag_start.elapsed()
            } else {
                Duration::ZERO
            };
            (
                rust.sessions_done + 1,
                self.kind_label(),
                rust.capture_us,
                rust.frame_start,
                rust.drag_steps,
                rust.drag_ticks,
                (drag_wall.as_micros() as f64 / 16_000.0).round() as usize,
            )
        };
        let frame_ms: Vec<f64> = self.rust().frame_ms[frame_start..].to_vec();
        let avg = if frame_ms.is_empty() {
            0.0
        } else {
            frame_ms.iter().sum::<f64>() / frame_ms.len() as f64
        };
        let worst = frame_ms.iter().copied().fold(0.0f64, f64::max);
        let reveal = *self.rust().reveal_ms.last().unwrap_or(&0.0);

        // P6: while the mask is still up, grab the screen again and measure the dim
        // on pixels. A variant that compiles but paints nothing would otherwise read
        // as a clean pass. First session only, so the cost never lands in a table.
        if index == 1 {
            let (hx, hy, hw, hh, vw, vh) = {
                let rust = self.rust();
                (
                    rust.hole_x,
                    rust.hole_y,
                    rust.hole_w,
                    rust.hole_h,
                    rust.viewport_w,
                    rust.viewport_h,
                )
            };
            let (fw, fh) = (qobject::frame_width(), qobject::frame_height());
            if fw > 0 && fh > 0 && vw > 0.0 && vh > 0.0 {
                let (sx, sy) = (fw as f64 / vw, fh as f64 / vh);
                let _ = qobject::capture();
                let check = qobject::self_check(
                    (hx * sx) as i32,
                    (hy * sy) as i32,
                    ((hx + hw) * sx) as i32,
                    ((hy + hh) * sy) as i32,
                );
                println!("[P1]   dim-check {check}");
                let grab = qobject::window_check(
                    (hx * sx) as i32,
                    (hy * sy) as i32,
                    ((hx + hw) * sx) as i32,
                    ((hy + hh) * sy) as i32,
                );
                println!("[P1]   dim-check {grab}");
            }
        }

        self.as_mut().set_mask_visible(false);
        self.as_mut().set_state(0);
        {
            let rust = self.as_mut().rust_mut().get_mut();
            rust.sessions_done = index;
            rust.next_start = Instant::now() + Duration::from_millis(400);
            rust.ticks_seen += ticks;
            rust.ticks_expected += expected_ticks;
            rust.steps_seen += steps;
            if !completed {
                // Discard a partial drag: its frame intervals were never a full run.
                rust.frame_ms.truncate(frame_start);
            }
        }

        if completed {
            println!(
                "[P1] session {index} {kind} reveal={reveal:.1}ms capture={capture_us}us dragSteps={steps} presents={} ticks={ticks}/{expected_ticks} frameAvg={avg:.2}ms frameWorst={worst:.2}ms",
                frame_ms.len()
            );
        } else {
            println!(
                "[P1] session {index} {kind} DID NOT COMPLETE (capture={capture_us}us dragSteps={steps} ticks={ticks}/{expected_ticks})"
            );
        }

        if self.rust().sessions_done >= self.rust().sessions_planned {
            self.conclude();
        }
    }

    fn conclude(mut self: Pin<&mut Self>) {
        let reveal_ms = self.rust().reveal_ms.clone();
        let frame_ms = self.rust().frame_ms.clone();
        let kinds: Vec<&'static str> = self.rust().reveal_kind.clone();
        let provider = format!(
            "calls={} avg={}us worst={}us installed={}",
            qobject::provider_calls(),
            qobject::provider_avg_us(),
            qobject::provider_worst_us(),
            qobject::provider_installed()
        );
        let describe = qobject::describe().to_string();
        let (ticks_seen, ticks_expected, steps_seen) = {
            let rust = self.rust();
            (rust.ticks_seen, rust.ticks_expected, rust.steps_seen)
        };

        let mut lines = vec![format!(
            "[P1] REVEAL n={} p50={:.1}ms p95={:.1}ms max={:.1}ms  (pass line: <=150ms)",
            reveal_ms.len(),
            percentile(&reveal_ms, 0.50),
            percentile(&reveal_ms, 0.95),
            percentile(&reveal_ms, 1.00)
        )];
        for (i, ms) in reveal_ms.iter().enumerate() {
            let kind = kinds.get(i).copied().unwrap_or("?");
            lines.push(format!("[P1]   reveal#{} {kind} = {ms:.1}ms", i + 1));
        }
        lines.push(format!(
            "[P1] DRAG-FRAME n={} p50={:.2}ms p95={:.2}ms max={:.2}ms  (pass line: <=8ms)",
            frame_ms.len(),
            percentile(&frame_ms, 0.50),
            percentile(&frame_ms, 0.95),
            percentile(&frame_ms, 1.00)
        ));
        lines.push(format!(
            "[P1] slack: 16ms pacer fired {ticks_seen}/{ticks_expected}; {steps_seen} drag steps -> {} presents",
            frame_ms.len()
        ));
        lines.push(format!("[P1] provider {provider}"));
        let (dim, shader_status) = {
            let rust = self.rust();
            (
                if rust.shader_dim { "shader" } else { "rects" },
                rust.shader_status,
            )
        };
        lines.push(format!("[P1] DIM {dim} shaderStatus={shader_status} (-1 means the shader was never instantiated)"));        lines.push(format!("[P1] env {describe}"));
        let report = lines.join("\n");
        println!("{report}");
        self.as_mut().rust_mut().get_mut().finished = true;
        let text = QString::from(&*report);
        self.report_ready(&text);
    }
}
