//! The spike QObject: exercises the three crossings that matter for the real app.
//!
//! QML -> Rust method call, Rust -> QML property change, Rust -> QML signal.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    // The thin C++ shim. Nothing here is reachable from Rust otherwise.
    unsafe extern "C++" {
        include!("hello-cxxqt/window_probe.h");

        /// className / winId / GWL_STYLE / GWL_EXSTYLE / opacity / alphaBuffer
        /// for every top-level window.
        #[rust_name = "probe_all_windows"]
        fn probeAllWindows() -> QString;

        /// QSurfaceFormat::setDefaultFormat with alphaBuffer(true).
        #[rust_name = "enable_alpha_buffer_by_default"]
        fn enableAlphaBufferByDefault();
    }

    // P5: the Qt install itself is a dependency, so its capabilities get audited
    // like one. Headless -- nothing here needs a window.
    unsafe extern "C++" {
        include!("hello-cxxqt/audit_source.h");

        /// QImageReader/QImageWriter format lists, plugin dirs and the shader
        /// toolchain, as one multi-line report.
        #[rust_name = "audit_qt_capabilities"]
        fn auditQtCapabilities() -> QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(i32, counter)]
        #[qproperty(i32, unobserved)]
        #[qproperty(QString, last_from_qml)]
        type Probe = super::ProbeRust;

        /// QML -> Rust: bump the counter and push a signal back out.
        #[qinvokable]
        #[cxx_name = "bumpCounter"]
        fn bump_counter(self: Pin<&mut Self>);

        /// QML -> Rust with a QString argument across the boundary.
        #[qinvokable]
        #[cxx_name = "recordFromQml"]
        fn record_from_qml(self: Pin<&mut Self>, message: &QString);

        /// Empty invokable, so the QML side can time the raw call crossing.
        #[qinvokable]
        #[cxx_name = "noop"]
        fn noop(self: Pin<&mut Self>);

        /// Timed property writes. `counter` is bound to a QML Label, `unobserved`
        /// is not, so the delta attributes cost between the boundary and QML
        /// binding re-evaluation. Feeds plan rule 4 ("hot path must not cross").
        #[qinvokable]
        #[cxx_name = "measureRoundTrips"]
        fn measure_round_trips(self: Pin<&mut Self>, iterations: i32);

        /// Called back from the QML signal handler, so a print here proves the
        /// Rust -> QML signal actually landed (the handler only runs if it did).
        #[qinvokable]
        #[cxx_name = "confirmSignal"]
        fn confirm_signal(self: Pin<&mut Self>, value: i32);

        /// Ask the C++ shim what Qt really put on the native window.
        #[qinvokable]
        #[cxx_name = "probeWindows"]
        fn probe_windows(self: Pin<&mut Self>);

        #[qsignal]
        #[cxx_name = "counterBumped"]
        fn counter_bumped(self: Pin<&mut Self>, value: i32, us_per_set: i64);
    }
}

use core::pin::Pin;
use cxx_qt_lib::QString;
use std::time::Instant;

#[derive(Default)]
pub struct ProbeRust {
    counter: i32,
    unobserved: i32,
    last_from_qml: QString,
}

impl qobject::Probe {
    pub fn bump_counter(mut self: Pin<&mut Self>) {
        let next = *self.counter() + 1;
        self.as_mut().set_counter(next);
        println!("[rust] bump_counter -> counter={next}, emitting counterBumped");
        self.counter_bumped(next, 0);
    }

    pub fn record_from_qml(mut self: Pin<&mut Self>, message: &QString) {
        let text = message.to_string();
        println!("[rust] got QString from QML: {text:?}");
        self.as_mut().set_last_from_qml(QString::from(&*text));
    }

    pub fn noop(self: Pin<&mut Self>) {}

    pub fn measure_round_trips(mut self: Pin<&mut Self>, iterations: i32) {
        let iterations = iterations.max(1) as i64;

        let start = Instant::now();
        for i in 1..=iterations {
            self.as_mut().set_counter(i as i32);
        }
        let bound = start.elapsed().as_micros() as i64;

        let start = Instant::now();
        for i in 1..=iterations {
            self.as_mut().set_unobserved(i as i32);
        }
        let unbound = start.elapsed().as_micros() as i64;

        println!(
            "[rust] {iterations} qproperty writes: bound-to-QML = {} us/write, unobserved = {} us/write",
            bound / iterations,
            unbound / iterations
        );
        self.counter_bumped(iterations as i32, bound / iterations);
    }

    pub fn confirm_signal(self: Pin<&mut Self>, value: i32) {
        println!("[rust] confirmSignal({value}) -> Rust -> QML signal crossing closed");
    }

    pub fn probe_windows(mut self: Pin<&mut Self>) {
        let report = qobject::probe_all_windows().to_string();
        println!("[shim]{report}");
        self.as_mut().set_last_from_qml(QString::from(&*report));
    }
}
