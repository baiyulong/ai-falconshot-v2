//! The session controller: how many pins there are, which one is at which slot,
//! and the only door through which new pins arrive or leave.
//!
//! Membership changes go through here because `count` is a qproperty and the QML
//! `Repeater` is what creates and destroys the windows. Everything else - a wheel
//! turn, a drag, a rotation - is per-pin view state and lives in
//! [`crate::pin_view`].

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qbytearray.h");
        type QByteArray = cxx_qt_lib::QByteArray;
    }

    // The product's thin C++ shim (cpp/pin_shim.h). QSurfaceFormat, QWindow::winId,
    // the Win32 extended styles and the QQuickImageProvider subclass are all
    // C++-only, which is why plan §3.6 declares a shim mandatory.
    unsafe extern "C++" {
        include!("qt_bridge/pin_shim.h");

        /// Must run before the first window or the pins lose their alpha.
        #[rust_name = "enable_alpha_buffer_by_default"]
        fn enableAlphaBufferByDefault();

        #[rust_name = "pin_store_frame"]
        fn pinStoreFrame(id: i64, png: &QByteArray);
        #[rust_name = "pin_drop_frame"]
        fn pinDropFrame(id: i64);
        #[rust_name = "pin_self_check"]
        fn pinSelfCheck(id: i64) -> QString;
        #[rust_name = "pin_pixel"]
        fn pinPixel(id: i64, x: i32, y: i32) -> QString;

        #[rust_name = "pin_desktop_x"]
        fn pinDesktopX() -> i32;
        #[rust_name = "pin_desktop_y"]
        fn pinDesktopY() -> i32;
        #[rust_name = "pin_desktop_w"]
        fn pinDesktopW() -> i32;
        #[rust_name = "pin_desktop_h"]
        fn pinDesktopH() -> i32;
        #[rust_name = "pin_primary_x"]
        fn pinPrimaryX() -> i32;
        #[rust_name = "pin_primary_y"]
        fn pinPrimaryY() -> i32;
        #[rust_name = "pin_primary_w"]
        fn pinPrimaryW() -> i32;
        #[rust_name = "pin_primary_h"]
        fn pinPrimaryH() -> i32;
        #[rust_name = "pin_device_pixel_ratio"]
        fn pinDevicePixelRatio() -> f64;

        #[rust_name = "pin_window_report"]
        fn pinWindowReport() -> QString;
        #[rust_name = "pin_install_message_capture"]
        fn pinInstallMessageCapture();
        #[rust_name = "pin_messages"]
        fn pinMessages() -> QString;
        #[rust_name = "pin_quit_after"]
        fn pinQuitAfter(ms: i32);
    }

    extern "RustQt" {
        /// How many pins are on screen - the QML Repeater's model.
        #[qobject]
        #[qml_element]
        #[qproperty(i32, count)]
        type Session = super::SessionRust;

        /// Re-read the state machine. QML calls this once at start-up and every
        /// mutation below also does it, so the count can never lag the set.
        #[qinvokable]
        fn refresh(self: Pin<&mut Self>);

        /// Called by `main.qml` once its root is built. See [`crate::state::qml_loaded`].
        #[qinvokable]
        #[cxx_name = "markLoaded"]
        fn mark_loaded(self: &Self);

        /// The pin in slot `index` of the z order, or 0 when there is none.
        #[qinvokable]
        #[cxx_name = "pinId"]
        fn pin_id(self: Pin<&mut Self>, index: i32) -> i64;

        /// §5.8's "pin this" before the capture service exists: put a checkerboard
        /// on the screen so the window path can be exercised on its own.
        #[qinvokable]
        #[cxx_name = "addDemo"]
        fn add_demo(self: Pin<&mut Self>, width: i32, height: i32) -> i64;

        /// §5.9.18 "关闭" from a pin window's own menu.
        #[qinvokable]
        #[cxx_name = "closePin"]
        fn close_pin(self: Pin<&mut Self>, id: i64) -> i32;
    }
}

use core::pin::Pin;

use falcon_core::geometry::PhysPoint;

use crate::state;

#[derive(Default)]
pub struct SessionRust {
    count: i32,
}

impl qobject::Session {
    pub fn refresh(mut self: Pin<&mut Self>) {
        let n = state::with(|s| s.ids().len());
        self.as_mut().set_count(n as i32);
    }

    pub fn pin_id(self: Pin<&mut Self>, index: i32) -> i64 {
        state::with(|s| s.id_at(index.max(0) as usize)) as i64
    }

    pub fn mark_loaded(&self) {
        state::mark_qml_loaded();
    }

    pub fn add_demo(mut self: Pin<&mut Self>, width: i32, height: i32) -> i64 {
        let frame = state::demo_frame(width.max(4) as u32, height.max(4) as u32);
        // New pins land a little offset from the last one, so a stack of them is a
        // stack of visible edges rather than one pin with three copies of itself.
        let step = state::with(|s| s.ids().len()).min(24) as i32;
        let at = PhysPoint::new(80 + step * 16, 80 + step * 16);
        match state::with(|s| s.add(frame, at)) {
            Ok(id) => {
                self.as_mut().refresh();
                id as i64
            }
            Err(e) => {
                eprintln!("[session] add_demo failed: {e}");
                0
            }
        }
    }

    pub fn close_pin(mut self: Pin<&mut Self>, id: i64) -> i32 {
        let n = state::with(|s| s.close(&[id as u64])) as i32;
        self.as_mut().refresh();
        n
    }
}

/// The shim, in Rust shapes. Everything the state machine needs from Qt goes
/// through here, so `state.rs` never has to name a C++ type - and the ids stay
/// `PinId`, which is a `u64`, rather than being cast at every call site.
pub mod shim {
    use cxx_qt_lib::QByteArray;
    use falcon_core::pin::PinId;

    use super::qobject;

    pub fn store_frame(id: PinId, png: &[u8]) {
        // A real deep copy: the Vec is Rust's again the moment the call ends.
        let mut ba = QByteArray::default();
        ba.resize(png.len() as isize);
        ba.as_mut_slice().copy_from_slice(png);
        qobject::pin_store_frame(id as i64, &ba);
    }

    pub fn drop_frame(id: PinId) {
        qobject::pin_drop_frame(id as i64);
    }

    pub fn self_check(id: PinId) -> String {
        qobject::pin_self_check(id as i64).to_string()
    }

    pub fn pixel(id: PinId, x: i32, y: i32) -> String {
        qobject::pin_pixel(id as i64, x, y).to_string()
    }

    pub fn desktop_bounds() -> (i32, i32, i32, i32) {
        (
            qobject::pin_desktop_x(),
            qobject::pin_desktop_y(),
            qobject::pin_desktop_w(),
            qobject::pin_desktop_h(),
        )
    }

    pub fn primary_bounds() -> (i32, i32, i32, i32) {
        (
            qobject::pin_primary_x(),
            qobject::pin_primary_y(),
            qobject::pin_primary_w(),
            qobject::pin_primary_h(),
        )
    }

    pub fn device_pixel_ratio() -> f64 {
        qobject::pin_device_pixel_ratio()
    }

    pub fn window_report() -> String {
        qobject::pin_window_report().to_string()
    }

    pub fn install_message_capture() {
        qobject::pin_install_message_capture();
    }

    pub fn messages() -> String {
        qobject::pin_messages().to_string()
    }

    pub fn quit_after(ms: i32) {
        qobject::pin_quit_after(ms);
    }
}
