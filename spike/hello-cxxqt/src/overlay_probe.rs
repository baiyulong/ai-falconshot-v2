// Spike increment 5: the P4 annotation-overlay probe.
//
// Question the plan asks (§4 P4): with the annotation layer as a texture, how long
// does "commit one 图元 -> it is on screen" take at 4K, and does the dirty-rect idea
// even survive QML? Pass line: <=16 ms.
//
// The layer is rasterized by Rust into a Vec<u8>; C++ publishes a non-owning view and
// the QQuickImageProvider serves either the whole layer or just the dirty rect. Every
// timestamp is Rust-side, so the numbers are reproducible without a human mouse.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    // The P4 shim. The surface pointer is why this block is `unsafe extern`: the
    // pixels stay owned by Rust and only the view crosses.
    //
    // Every #[rust_name] here carries an `overlay_`/`publish` prefix even though the
    // C++ name is already unique. cxx keys the generated bridge symbols on the *Rust*
    // name, so a second bridge file in the same crate that reuses e.g. `describe`
    // links with LNK2005 "already defined" against mask_probe.rs.
    unsafe extern "C++" {
        include!("hello-cxxqt/overlay_source.h");

        /// Publish the Rust-owned overlay surface. Non-owning: Rust keeps the Vec alive.
        /// `unsafe fn` because a raw pointer crosses; cxx will not let a safe
        /// declaration take one, even inside an `unsafe extern "C++"` block.
        #[rust_name = "publish"]
        unsafe fn overlayPublish(data: *const u8, width: i32, height: i32);

        #[rust_name = "overlay_provider_calls"]
        fn overlayProviderCalls() -> i64;
        #[rust_name = "overlay_provider_avg_us"]
        fn overlayProviderAvgUs() -> i64;
        #[rust_name = "overlay_provider_worst_us"]
        fn overlayProviderWorstUs() -> i64;
        #[rust_name = "overlay_provider_bytes"]
        fn overlayProviderBytes() -> i64;
        #[rust_name = "overlay_describe"]
        fn overlayDescribe() -> QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        /// 0 = idle (a submit is due), 1 = waiting for the matching present,
        /// 2 = the whole-layer release of `qml-whole-clear` is in flight.
        #[qproperty(i32, state)]
        /// Unique per submit; it is what forces the Image to re-request.
        #[qproperty(i32, token)]
        #[qproperty(bool, running)]
        #[qproperty(bool, window_visible)]
        /// Which layer variant QML should be showing right now.
        #[qproperty(bool, image_mode)]
        #[qproperty(bool, painted_mode)]
        #[qproperty(bool, rect_mode)]
        /// The bump that makes PaintedOverlay::setSeq call update().
        #[qproperty(i32, paint_token)]
        /// Dirty rect in canvas pixels; w/h of 0 means "re-request the whole layer".
        #[qproperty(i32, rect_x)]
        #[qproperty(i32, rect_y)]
        #[qproperty(i32, rect_w)]
        #[qproperty(i32, rect_h)]
        #[qproperty(i32, canvas_w)]
        #[qproperty(i32, canvas_h)]
        #[qproperty(i32, config_done)]
        #[qproperty(i32, config_planned)]
        type OverlayProbe = super::OverlayProbeRust;

        #[qinvokable]
        #[cxx_name = "runSuite"]
        fn run_suite(self: Pin<&mut Self>, quick: bool);

        #[qinvokable]
        #[cxx_name = "tick"]
        fn tick(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "noteSwap"]
        fn note_swap(self: Pin<&mut Self>);

        /// Same reason as P1's reportReady: a QString field would make the backing
        /// struct !Unpin and every field write would need Pin projection.
        #[qsignal]
        #[cxx_name = "reportReady"]
        fn report_ready(self: Pin<&mut Self>, report: &QString);
    }
}

use core::pin::Pin;
use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use std::time::{Duration, Instant};

/// Submits spaced this far apart. Deliberately not a divisor of the ~16.6 ms vsync,
/// so the phase of each submit against the next vsync boundary varies and the
/// latency distribution spreads instead of aliasing onto one value.
const SUBMIT_INTERVAL_MS: u64 = 13;
/// The present is late by this much: worth a line, worth keeping.
const STALL_MS: u64 = 800;
/// ... and a config whose frame is still absent after this is not measurable, so the
/// suite moves on with whatever samples it did collect.
const GIVE_UP_MS: u64 = 3200;
const VSYNC_NOMINAL_MS: f64 = 16.667;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Upload {
    /// Re-request the whole layer: the only thing a plain QML Image can do.
    Whole,
    /// The same whole layer, but served as a deep Qt-owned copy. Same bytes,
    /// different buffer ownership - the discriminator for the whole-layer stall.
    WholeCopy,
    /// Whole layer again, but released first: `source` goes empty for one beat, then
    /// the new URL is committed. This is the only structural difference between P4's
    /// failing configs and P1's working one, so it gets its own row.
    WholeClear,
    /// Ask the provider for the dirty rect only (a partial upload's worth of bytes).
    Dirty,
    /// QQuickPaintedItem: CPU raster of the whole item + whole upload.
    Painted,
    /// A plain QML Rectangle - geometry change, no texture at all. The anchor that
    /// says what "表达成 item 而不是位图" is worth.
    ItemRect,
}

impl Upload {
    fn label(self) -> &'static str {
        match self {
            Upload::Whole => "qml-whole-layer",
            Upload::WholeCopy => "qml-whole-copy",
            Upload::WholeClear => "qml-whole-clear",
            Upload::Dirty => "qml-dirty-rect",
            Upload::Painted => "painteditem-fallback",
            Upload::ItemRect => "plain-rectangle",
        }
    }
}

#[derive(Clone, Copy)]
struct Config {
    name: &'static str,
    canvas: (i32, i32),
    /// The 图元 bounding box actually rasterized. Note this is independent of the
    /// upload volume: "whole layer" still only rasterizes the bbox.
    bbox: (i32, i32, i32, i32),
    upload: Upload,
    /// How far apart commits are paced. The fast configs pace below one vsync so the
    /// variant is stressed harder than any user could; one 250 ms config says what
    /// happens at a realistic 图元-per-keystroke rate instead.
    pace_ms: u64,
}

const CONFIGS: [Config; 10] = [
    Config {
        name: "4k-whole",
        canvas: (3840, 2160),
        bbox: (400, 300, 400, 300),
        upload: Upload::Whole,
        pace_ms: SUBMIT_INTERVAL_MS,
    },
    Config {
        // The same layer re-requested at a rate a human can actually produce. If the
        // fast config's stall is churn and not the reload model, this is where it
        // stops reproducing.
        name: "4k-whole-250ms",
        canvas: (3840, 2160),
        bbox: (400, 300, 400, 300),
        upload: Upload::Whole,
        pace_ms: 250,
    },
    Config {
        // The A/B for the row above: same bytes, same pace, but each commit first
        // releases the previous texture (P1 does that implicitly by hiding the mask
        // between sessions). If this one renders, the fix is the handshake, not a
        // new shim.
        name: "4k-whole-clear",
        canvas: (3840, 2160),
        bbox: (400, 300, 400, 300),
        upload: Upload::WholeClear,
        pace_ms: 250,
    },
    Config {
        name: "4k-whole-copy",
        canvas: (3840, 2160),
        bbox: (400, 300, 400, 300),
        upload: Upload::WholeCopy,
        pace_ms: SUBMIT_INTERVAL_MS,
    },
    Config {
        name: "4k-dirty-400x300",
        canvas: (3840, 2160),
        bbox: (400, 300, 400, 300),
        upload: Upload::Dirty,
        pace_ms: SUBMIT_INTERVAL_MS,
    },
    Config {
        name: "4k-dirty-1600x1000",
        canvas: (3840, 2160),
        bbox: (900, 600, 1600, 1000),
        upload: Upload::Dirty,
        pace_ms: SUBMIT_INTERVAL_MS,
    },
    Config {
        name: "native-whole",
        canvas: (3072, 1920),
        bbox: (400, 300, 400, 300),
        upload: Upload::Whole,
        pace_ms: SUBMIT_INTERVAL_MS,
    },
    Config {
        name: "native-dirty-400x300",
        canvas: (3072, 1920),
        bbox: (400, 300, 400, 300),
        upload: Upload::Dirty,
        pace_ms: SUBMIT_INTERVAL_MS,
    },
    Config {
        name: "painted-window",
        canvas: (3072, 1920),
        bbox: (400, 300, 400, 300),
        upload: Upload::Painted,
        pace_ms: SUBMIT_INTERVAL_MS,
    },
    // The reference row: same window, no bitmap at all.
    Config {
        name: "plain-rectangle",
        canvas: (3072, 1920),
        bbox: (400, 300, 400, 300),
        upload: Upload::ItemRect,
        pace_ms: SUBMIT_INTERVAL_MS,
    },
];

pub struct OverlayProbeRust {
    state: i32,
    token: i32,
    running: bool,
    window_visible: bool,
    image_mode: bool,
    painted_mode: bool,
    rect_mode: bool,
    paint_token: i32,
    rect_x: i32,
    rect_y: i32,
    rect_w: i32,
    rect_h: i32,
    canvas_w: i32,
    canvas_h: i32,
    config_done: i32,
    config_planned: i32,

    /// The layer itself. Kept alive (and never handed to the provider as a
    /// dangling view) by pushing superseded buffers into `kept` instead of freeing.
    overlay: Vec<u8>,
    kept: Vec<Vec<u8>>,
    plan: Vec<Config>,
    next_submit: Instant,
    t0: Instant,
    /// Phase 1 of `Upload::WholeClear` started here; phase 2 runs once a frame has
    /// actually been swapped with the layer released (or after a timeout).
    clear_at: Instant,
    swap_after_clear: bool,
    raster_us: i64,
    submits: usize,
    submits_per_config: usize,
    samples_ms: Vec<f64>,
    raster_us_list: Vec<i64>,
    /// One summary line per completed config, so the on-screen report is the table.
    lines: Vec<String>,
    /// Submits in the current config whose present blew past STALL_MS.
    stalled: usize,
    stall_flagged: bool,
    /// ... and how many of them in an unbroken row. Three is enough to say the
    /// variant cannot hold a frame budget at all.
    stall_run: usize,
    started: bool,
    finished: bool,
}

impl Default for OverlayProbeRust {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            state: 0,
            token: 0,
            running: false,
            window_visible: false,
            image_mode: false,
            painted_mode: false,
            rect_mode: false,
            paint_token: 0,
            rect_x: 0,
            rect_y: 0,
            rect_w: 0,
            rect_h: 0,
            canvas_w: 0,
            canvas_h: 0,
            config_done: 0,
            config_planned: 0,
            overlay: Vec::new(),
            kept: Vec::new(),
            plan: Vec::new(),
            next_submit: now,
            t0: now,
            clear_at: now,
            swap_after_clear: false,
            raster_us: 0,
            submits: 0,
            submits_per_config: 24,
            samples_ms: Vec::new(),
            raster_us_list: Vec::new(),
            lines: Vec::new(),
            stalled: 0,
            stall_flagged: false,
            stall_run: 0,
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

/// Draw one 图元 into the ARGB32 (memory order B,G,R,A) layer: a 3 px outline plus
/// one diagonal, so the Rust path and the QPainter fallback do comparable work.
fn rasterize_marker(buf: &mut [u8], canvas_w: i32, canvas_h: i32, rect: (i32, i32, i32, i32), color: [u8; 3]) {
    let stride = canvas_w as usize * 4;
    let put = |buf: &mut [u8], x: i32, y: i32| {
        if x < 0 || y < 0 || x >= canvas_w || y >= canvas_h {
            return;
        }
        let at = y as usize * stride + x as usize * 4;
        if at + 3 >= buf.len() {
            return;
        }
        buf[at] = color[2];
        buf[at + 1] = color[1];
        buf[at + 2] = color[0];
        buf[at + 3] = 255;
    };

    let (x0, y0, w, h) = rect;
    let (x1, y1) = (x0 + w - 1, y0 + h - 1);
    for t in 0..3i32 {
        for x in x0..=x1 {
            put(buf, x, y0 + t);
            put(buf, x, y1 - t);
        }
        for y in y0..=y1 {
            put(buf, x0 + t, y);
            put(buf, x1 - t, y);
        }
    }

    // Bresenham-ish diagonal, stepped so it cannot loop forever on a degenerate box.
    let steps = (w.max(h) * 2).max(1);
    for i in 0..=steps {
        let f = i as f64 / steps as f64;
        put(buf, (x0 as f64 + f * (x1 - x0) as f64) as i32, (y0 as f64 + f * (y1 - y0) as f64) as i32);
    }
}

impl qobject::OverlayProbe {
    pub fn run_suite(mut self: Pin<&mut Self>, quick: bool) {
        {
            let rust = self.as_mut().rust_mut().get_mut();
            if rust.started {
                return;
            }
            rust.started = true;
            rust.submits_per_config = if quick { 12 } else { 24 };
            rust.plan = CONFIGS.to_vec();
            rust.config_planned = rust.plan.len() as i32;
            rust.next_submit = Instant::now() + Duration::from_millis(200);
            let planned = rust.plan.len();
            println!("[P4] suite start: {planned} configs x {} submits", rust.submits_per_config);
        }
        self.as_mut().set_running(true);
        self.as_mut().set_window_visible(true);
    }

    /// The pacer. Submits are serialized on their own present, so each sample is
    /// exactly one 图元 commit -> one swapped frame.
    pub fn tick(mut self: Pin<&mut Self>) {
        if self.rust().finished || !self.rust().running {
            return;
        }
        if self.rust().state == 2 {
            let released = self.rust().swap_after_clear
                || self.rust().clear_at.elapsed() > Duration::from_millis(120);
            if released {
                self.commit_whole_clear();
            }
            return;
        }
        if self.rust().state == 1 {
            let elapsed = self.rust().t0.elapsed();
            if elapsed > Duration::from_millis(STALL_MS) && !self.rust().stall_flagged {
                let submits = self.rust().submits;
                let name = self.config().map(|c| c.name).unwrap_or("?");
                println!("[P4]   submit {submits}/{} {name}: no present at {}ms", self.rust().submits_per_config, elapsed.as_millis());
                self.as_mut().rust_mut().get_mut().stall_flagged = true;
                self.as_mut().rust_mut().get_mut().stalled += 1;
            }
            if elapsed > Duration::from_millis(GIVE_UP_MS) {
                let name = self.config().map(|c| c.name).unwrap_or("?");
                let (submits, planned, raster_us) = {
                    let rust = self.rust();
                    (rust.submits, rust.submits_per_config, rust.raster_us)
                };
                println!("[P4]   submit {submits}/{planned} {name}: NO PRESENT by {GIVE_UP_MS}ms (counted as {GIVE_UP_MS}ms)");
                let give_up = {
                    let rust = self.as_mut().rust_mut().get_mut();
                    rust.samples_ms.push(GIVE_UP_MS as f64);
                    if raster_us >= 0 {
                        rust.raster_us_list.push(raster_us);
                    }
                    rust.stall_run += 1;
                    rust.state = 0;
                    rust.next_submit = Instant::now() + Duration::from_millis(SUBMIT_INTERVAL_MS);
                    rust.stall_run >= 3
                };
                if give_up {
                    println!("[P4] three presents missing in a row -> leaving this config");
                    self.advance_config(false);
                } else if self.rust().submits >= planned {
                    self.advance_config(true);
                }
            }
            return;
        }
        if Instant::now() >= self.rust().next_submit {
            self.submit();
        }
    }

    fn config(&self) -> Option<Config> {
        self.rust().plan.get(self.rust().config_done as usize).copied()
    }

    fn submit(mut self: Pin<&mut Self>) {
        let Some(cfg) = self.config() else {
            self.as_mut().rust_mut().get_mut().finished = true;
            return;
        };
        let rect = self.marker_rect(cfg);

        // `qml-whole-clear` takes two beats: drop the current texture, then commit the
        // new URL on the next tick. Everything else commits in one go.
        if cfg.upload == Upload::WholeClear {
            self.as_mut().set_image_mode(false);
            let now = Instant::now();
            let rust = self.as_mut().rust_mut().get_mut();
            rust.state = 2;
            rust.clear_at = now;
            rust.swap_after_clear = false;
            return;
        }

        // t0 is the equivalent of the mouse releasing the 图元: rasterization is
        // inside the measured window, not outside it.
        let t0 = Instant::now();

        let raster_us = match cfg.upload {
            Upload::Whole | Upload::WholeCopy | Upload::Dirty | Upload::WholeClear => {
                self.as_mut().rasterize_into_place(cfg, rect)
            }
            Upload::Painted | Upload::ItemRect => -1,
        };

        let (token, paint_token, image_mode, painted_mode, rect_mode) = {
            let rust = self.as_mut().rust_mut().get_mut();
            rust.token += 1;
            rust.paint_token += 1;
            rust.t0 = t0;
            rust.state = 1;
            rust.stall_flagged = false;
            rust.submits += 1;
            rust.raster_us = raster_us;
            rust.stall_flagged = false;
            if raster_us >= 0 {
                rust.raster_us_list.push(raster_us);
            }
            (rust.token, rust.paint_token, matches!(cfg.upload, Upload::Whole | Upload::WholeCopy | Upload::Dirty | Upload::WholeClear), cfg.upload == Upload::Painted, cfg.upload == Upload::ItemRect)
        };

        // The Image variants are the only ones that ask the provider for the whole
        // layer; everything else - dirty rect, painted marker, Rectangle geometry -
        // is committed at the 图元's own rect.
        match cfg.upload {
            // w/h = 0 is the provider's "give me the whole layer, shallow" marker.
            Upload::Whole => {
                self.as_mut().set_rect_x(0);
                self.as_mut().set_rect_y(0);
                self.as_mut().set_rect_w(0);
                self.as_mut().set_rect_h(0);
            }
            // Asking for the full rect instead takes the same bytes through the
            // provider's deep-copy branch: Qt owns them, and that is the variable.
            Upload::WholeCopy => {
                self.as_mut().set_rect_x(0);
                self.as_mut().set_rect_y(0);
                self.as_mut().set_rect_w(cfg.canvas.0);
                self.as_mut().set_rect_h(cfg.canvas.1);
            }
            Upload::Dirty | Upload::Painted | Upload::ItemRect => {
                self.as_mut().set_rect_x(rect.0);
                self.as_mut().set_rect_y(rect.1);
                self.as_mut().set_rect_w(rect.2);
                self.as_mut().set_rect_h(rect.3);
            }
            // Never reached: the clear variant returns into its two-beat path above.
            Upload::WholeClear => {}
        }
        self.as_mut().set_image_mode(image_mode);
        self.as_mut().set_painted_mode(painted_mode);
        self.as_mut().set_rect_mode(rect_mode);
        self.as_mut().set_paint_token(paint_token);
        self.as_mut().set_token(token);
    }

    /// Where this submit's 图元 lands. Slid around so no submit is served by a warm
    /// pixel cache line, and shared by every upload variant so they draw the same
    /// amount of geometry.
    fn marker_rect(&self, cfg: Config) -> (i32, i32, i32, i32) {
        let (bx, by, bw, bh) = cfg.bbox;
        let span_x = (cfg.canvas.0 - bw).max(1);
        let span_y = (cfg.canvas.1 - bh).max(1);
        let index = self.rust().submits;
        (
            bx + ((index * 173) as i32 % span_x) / 2,
            by + ((index * 97) as i32 % span_y) / 2,
            bw,
            bh,
        )
    }

    /// Rasterize the 图元 into the Rust-owned layer. Returns microseconds spent
    /// rasterizing (the CPU half of the pass line). Only the texture variants use it;
    /// the painted and Rectangle variants get their pixels elsewhere.
    fn rasterize_into_place(mut self: Pin<&mut Self>, cfg: Config, rect: (i32, i32, i32, i32)) -> i64 {
        {
            let rust = self.as_mut().rust_mut().get_mut();
            if rust.canvas_w != cfg.canvas.0 || rust.canvas_h != cfg.canvas.1 {
                let len = (cfg.canvas.0 as usize) * (cfg.canvas.1 as usize) * 4;
                let mut fresh = vec![0u8; len];
                std::mem::swap(&mut rust.overlay, &mut fresh);
                // The provider may still hand out a shallow view of the old buffer,
                // so retire instead of free - the same shortcut P1 documented.
                rust.kept.push(fresh);
                rust.canvas_w = cfg.canvas.0;
                rust.canvas_h = cfg.canvas.1;
                let ptr = rust.overlay.as_ptr();
                unsafe { qobject::publish(ptr, cfg.canvas.0, cfg.canvas.1) };
            }
        }

        let started = Instant::now();
        {
            let rust = self.as_mut().rust_mut().get_mut();
            rasterize_marker(&mut rust.overlay, cfg.canvas.0, cfg.canvas.1, rect, [255, 40, 40]);
        }
        started.elapsed().as_micros() as i64
    }

    /// Phase 2 of `qml-whole-clear`: the layer is no longer referenced, so commit the
    /// new URL exactly the way the other Image variants do - rasterize inside the
    /// measured window, t0 before it - to keep the two rows comparable.
    fn commit_whole_clear(mut self: Pin<&mut Self>) {
        let Some(cfg) = self.config() else {
            self.as_mut().rust_mut().get_mut().finished = true;
            return;
        };
        let rect = self.marker_rect(cfg);
        let t0 = Instant::now();
        let raster_us = self.as_mut().rasterize_into_place(cfg, rect);
        let token = {
            let rust = self.as_mut().rust_mut().get_mut();
            rust.token += 1;
            rust.t0 = t0;
            rust.state = 1;
            rust.submits += 1;
            rust.raster_us = raster_us;
            rust.stall_flagged = false;
            if raster_us >= 0 {
                rust.raster_us_list.push(raster_us);
            }
            rust.token
        };
        self.as_mut().set_rect_x(0);
        self.as_mut().set_rect_y(0);
        self.as_mut().set_rect_w(0);
        self.as_mut().set_rect_h(0);
        self.as_mut().set_image_mode(true);
        self.as_mut().set_token(token);
    }

    pub fn note_swap(mut self: Pin<&mut Self>) {
        if self.rust().state == 2 {
            // The scene swapped with the layer released: phase 2 may run.
            self.as_mut().rust_mut().get_mut().swap_after_clear = true;
            return;
        }
        if self.rust().state != 1 {
            return;
        }
        let (elapsed_ms, raster_us, done, planned) = {
            let rust = self.rust();
            (
                rust.t0.elapsed().as_secs_f64() * 1000.0,
                rust.raster_us,
                rust.submits,
                rust.submits_per_config,
            )
        };
        let pace = self.config().map(|c| c.pace_ms).unwrap_or(SUBMIT_INTERVAL_MS);
        {
            let rust = self.as_mut().rust_mut().get_mut();
            rust.samples_ms.push(elapsed_ms);
            rust.stall_run = 0;
            rust.state = 0;
            rust.next_submit = Instant::now() + Duration::from_millis(pace);
        }
        println!(
            "[P4]   submit {done}/{planned} {} commit->present={elapsed_ms:.2}ms raster={raster_txt}",
            self.config().map(|c| c.name).unwrap_or("?"),
            raster_txt = if raster_us < 0 { "n/a".to_string() } else { format!("{raster_us}us") },
        );
        if done >= planned {
            self.advance_config(true);
        }
    }

    fn advance_config(mut self: Pin<&mut Self>, completed: bool) {
        let seq = self.rust().config_done;
        let name = self.config().map(|c| c.name).unwrap_or("?").to_string();
        let upload = self.config().map(|c| c.upload).unwrap_or(Upload::Whole);
        let raster = self.rust().raster_us_list.clone();
        let samples: Vec<f64> = self.rust().samples_ms.clone();
        let planned = self.rust().submits_per_config;
        let stalled = self.rust().stalled;
        let at = self.rust().submits;

        {
            let rust = self.as_mut().rust_mut().get_mut();
            rust.config_done = seq + 1;
            rust.submits = 0;
            rust.stalled = 0;
            rust.stall_run = 0;
            rust.samples_ms.clear();
            rust.raster_us_list.clear();
            rust.state = 0;
            rust.next_submit = Instant::now() + Duration::from_millis(250);
        }

        let mean = if samples.is_empty() {
            0.0
        } else {
            samples.iter().sum::<f64>() / samples.len() as f64
        };
        // Submits land at a varying phase against vsync, so the mean carries a
        // half-frame of pure alignment. What is left is the actual work.
        let work = (mean - VSYNC_NOMINAL_MS / 2.0).max(0.0);
        let slips = samples.iter().filter(|v| **v > VSYNC_NOMINAL_MS * 1.5).count();
        let raster_txt = if raster.is_empty() {
            // The variants that do not rasterize in Rust: the QPainter fallback and
            // the plain Rectangle pay their draw cost inside the frame.
            "raster=n/a".to_string()
        } else {
            let avg = raster.iter().sum::<i64>() / raster.len() as i64;
            let worst = raster.iter().copied().max().unwrap_or(0);
            format!("raster avg={avg}us worst={worst}us")
        };
        let tail = if completed {
            format!("stalled={stalled}")
        } else {
            format!("GAVE UP at submit {at}/{planned}, stalled={stalled}")
        };

        let line = format!(
            "[P4 cfg {seq}] {name:<22} {upload_label:<18} n={n}/{planned} commit->present p50={p50:.2} p90={p90:.2} max={max:.2}ms  mean={mean:.2}  work~{work:.2}ms  slips={slips}  {raster_txt}  {tail}",
            upload_label = upload.label(),
            n = samples.len(),
            p50 = percentile(&samples, 0.50),
            p90 = percentile(&samples, 0.90),
            max = percentile(&samples, 1.00),
        );
        println!("{line}");
        self.as_mut().rust_mut().get_mut().lines.push(line.clone());

        if self.rust().config_done >= self.rust().config_planned {
            self.conclude();
        }
    }

    fn conclude(mut self: Pin<&mut Self>) {
        let bytes = qobject::overlay_provider_bytes();
        let provider = format!(
            "calls={} avg={}us worst={}us servedMB={:.1}",
            qobject::overlay_provider_calls(),
            qobject::overlay_provider_avg_us(),
            qobject::overlay_provider_worst_us(),
            bytes as f64 / (1024.0 * 1024.0)
        );
        let describe = qobject::overlay_describe().to_string();
        let mut lines = vec![format!(
            "[P4] pass line: commit->present <=16ms at 4K (panel here is 60.0Hz, so one vsync = {:.2}ms)",
            VSYNC_NOMINAL_MS
        )];
        lines.extend(self.rust().lines.iter().cloned());
        lines.push(format!("[P4] provider {provider}"));
        lines.push(format!("[P4] env{describe}"));
        let report = lines.join("\n");
        println!("{report}");
        self.as_mut().rust_mut().get_mut().finished = true;
        self.as_mut().set_window_visible(false);
        self.as_mut().set_running(false);
        let text = QString::from(&*report);
        self.report_ready(&text);
    }
}
