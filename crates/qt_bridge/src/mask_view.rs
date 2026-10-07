//! One capture mask window's controller - the screen-sized surface that shows the
//! frozen desktop with a bright hole in it.
//!
//! The same shape as [`crate::pin_view`]: the decision lives in
//! [`crate::mask`], this object translates into and out of it, and QML binds. It is
//! a separate object because a mask is not a pin. A pin's picture is the user's and
//! lives as long as they keep it; a mask's picture is a screen-sized texture that
//! exists for one flow, one per monitor rather than one per content, and whose
//! only gesture is the selection.
//!
//! Nothing here re-reads the screen. `reload` reports the hole; [`crate::capture`]
//! produced the pixels before this window existed, which is the whole reason the
//! mask can be honest about what it is showing.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qbytearray.h");
        type QByteArray = cxx_qt_lib::QByteArray;
    }

    // The mask half of the shim (cpp/pin_shim.h). The frame store and the screen
    // list are both C++-only: the first because a texture has to live in Qt's
    // process, the second because a `Window` is placed in Qt's coordinates.
    unsafe extern "C++" {
        include!("qt_bridge/pin_shim.h");

        #[rust_name = "mask_store_raw"]
        fn maskStoreRaw(key: &QString, rgba: &QByteArray, width: i32, height: i32);
        #[rust_name = "mask_drop_frame"]
        fn maskDropFrame(key: &QString);
        #[rust_name = "mask_self_check"]
        fn maskSelfCheck(key: &QString) -> QString;

        #[rust_name = "pin_screen_count"]
        fn pinScreenCount() -> i32;
        #[rust_name = "pin_screen_name"]
        fn pinScreenName(index: i32) -> QString;
        #[rust_name = "pin_screen_x"]
        fn pinScreenX(index: i32) -> i32;
        #[rust_name = "pin_screen_y"]
        fn pinScreenY(index: i32) -> i32;
        #[rust_name = "pin_screen_w"]
        fn pinScreenW(index: i32) -> i32;
        #[rust_name = "pin_screen_h"]
        fn pinScreenH(index: i32) -> i32;
        #[rust_name = "pin_screen_device_pixel_ratio"]
        fn pinScreenDevicePixelRatio(index: i32) -> f64;
    }

    extern "RustQt" {
        /// One screen's mask. QML creates it inside `CaptureMask.qml` and binds the
        /// window rect, the hole and which dim path draws it off this.
        #[qobject]
        #[qml_element]
        #[qproperty(i32, index)]
        #[qproperty(bool, live)]
        #[qproperty(bool, shown)]
        #[qproperty(i32, win_x)]
        #[qproperty(i32, win_y)]
        #[qproperty(i32, win_w)]
        #[qproperty(i32, win_h)]
        #[qproperty(i32, hole_x)]
        #[qproperty(i32, hole_y)]
        #[qproperty(i32, hole_w)]
        #[qproperty(i32, hole_h)]
        #[qproperty(f64, dpr)]
        #[qproperty(f64, dim_alpha)]
        #[qproperty(bool, shader)]
        #[qproperty(i32, revision)]
        #[qproperty(QString, key)]
        #[qproperty(QString, name)]
        type MaskView = super::MaskViewRust;

        /// Which screen this window is. Called by the `Instantiator` after creation,
        /// because a delegate has no index of its own to read.
        #[qinvokable]
        #[cxx_name = "assign"]
        fn assign(self: Pin<&mut Self>, index: i32);

        /// Pull the mask state's current answer for `index`.
        #[qinvokable]
        fn reload(self: Pin<&mut Self>);

        /// `ShaderEffect.status`, kept so `--mask` can assert the value P6 measured
        /// for a compiled shader (`0`) instead of going to look for the `.qsb` in the
        /// package. `2` is Error, and an error here while the rectangle path still
        /// draws is exactly plan §9.1's hole.
        #[qinvokable]
        #[cxx_name = "noteShaderStatus"]
        fn note_shader_status(self: Pin<&mut Self>, status: i32);

        /// `Window.onFrameSwapped`. Only the first one is timed, and that number is
        /// the GPU warm-up a pre-warm has to move out of the hotkey path.
        #[qinvokable]
        #[cxx_name = "noteSwap"]
        fn note_swap(self: Pin<&mut Self>);

        /// The selection, in physical desktop pixels - the same space a pin's crop
        /// arrives in. Window-local device-independent pixels are derived per screen.
        #[qinvokable]
        #[cxx_name = "setHole"]
        fn set_hole(self: Pin<&mut Self>, x: i32, y: i32, w: i32, h: i32);

        /// Which of the two dim paths draws: one `ShaderEffect` pass, or four
        /// rectangles. The second is what `QT_QUICK_BACKEND=software` has to fall
        /// back to, which is why it is a switch rather than a deleted branch.
        ///
        /// `apply_` rather than `set_`: the `shader` qproperty already generated an
        /// associated function called `set_shader`, and cxx-qt refuses the bridge
        /// twice over if the invokable claims the same name.
        #[qinvokable]
        #[cxx_name = "applyShader"]
        fn apply_shader(self: Pin<&mut Self>, on: bool);
    }
}

use core::pin::Pin;
use cxx_qt_lib::QString;

use falcon_core::geometry::PhysRect;

use crate::mask;

#[derive(Default)]
pub struct MaskViewRust {
    index: i32,
    live: bool,
    shown: bool,
    win_x: i32,
    win_y: i32,
    win_w: i32,
    win_h: i32,
    hole_x: i32,
    hole_y: i32,
    hole_w: i32,
    hole_h: i32,
    dpr: f64,
    dim_alpha: f64,
    shader: bool,
    revision: i32,
    key: QString,
    name: QString,
}

impl qobject::MaskView {
    pub fn assign(mut self: Pin<&mut Self>, index: i32) {
        self.as_mut().set_index(index);
        self.as_mut().reload();
    }

    /// Copy the mask state's answer into the properties QML binds to.
    ///
    /// One lock, one read: the window rect, its hole and its texture key come from
    /// the same moment, so a hole can never be drawn against a frame from the
    /// previous flow.
    pub fn reload(mut self: Pin<&mut Self>) {
        let index = *self.index();
        let (view, shader, label) = mask::with(|m| {
            (
                m.view_data(index.max(0) as usize),
                m.shader,
                m.slot(index.max(0) as usize)
                    .map(|s| (s.key.clone(), s.name.clone())),
            )
        });

        self.as_mut().set_live(view.live);
        self.as_mut().set_shown(view.shown);
        self.as_mut().set_win_x(view.geom.x);
        self.as_mut().set_win_y(view.geom.y);
        self.as_mut().set_win_w(view.geom.w as i32);
        self.as_mut().set_win_h(view.geom.h as i32);
        self.as_mut().set_hole_x(view.hole.x);
        self.as_mut().set_hole_y(view.hole.y);
        self.as_mut().set_hole_w(view.hole.w as i32);
        self.as_mut().set_hole_h(view.hole.h as i32);
        self.as_mut().set_dpr(view.scale.ratio());
        // The dim the readback asserts against, in the one number QML draws with -
        // so `--mask` checking `1 - DIM` is checking the value actually on screen.
        self.as_mut().set_dim_alpha(mask::DIM);
        self.as_mut().set_shader(shader);
        self.as_mut().set_revision(view.revision as i32);
        match &label {
            Some((key, name)) => {
                self.as_mut().set_key(QString::from(&**key));
                self.as_mut().set_name(QString::from(&**name));
            }
            None => {
                self.as_mut().set_key(QString::default());
                self.as_mut().set_name(QString::default());
            }
        }
    }

    pub fn note_shader_status(self: Pin<&mut Self>, status: i32) {
        let index = *self.index();
        mask::with(|m| m.note_shader_status(index.max(0) as usize, status));
    }

    pub fn note_swap(self: Pin<&mut Self>) {
        let index = *self.index();
        mask::with(|m| m.note_swap(index.max(0) as usize));
    }

    pub fn set_hole(mut self: Pin<&mut Self>, x: i32, y: i32, w: i32, h: i32) {
        let rect = PhysRect::new(x, y, w.max(0) as u32, h.max(0) as u32);
        mask::with(|m| m.set_hole(rect));
        self.as_mut().reload();
    }

    pub fn apply_shader(mut self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.set_shader(on));
        self.as_mut().reload();
    }
}

/// The shim, in Rust shapes - the mask side. Kept next to the bridge that declares
/// it, because a `QString` argument only exists on this side of the boundary.
pub mod shim {
    use cxx_qt_lib::{QByteArray, QString};

    use super::qobject;

    /// One screen as Qt sees it, in the DIP space a `Window` is placed in.
    #[derive(Clone, Debug)]
    pub struct Screen {
        pub name: String,
        pub x: i32,
        pub y: i32,
        pub w: i32,
        pub h: i32,
        pub dpr: f64,
    }

    /// Publish one screen's frozen pixels under `key`.
    pub fn store_raw(key: &str, rgba: &[u8], width: u32, height: u32) {
        let mut ba = QByteArray::default();
        ba.resize(rgba.len() as isize);
        ba.as_mut_slice().copy_from_slice(rgba);
        let k = QString::from(key);
        qobject::mask_store_raw(&k, &ba, width as i32, height as i32);
    }

    pub fn drop_frame(key: &str) {
        let k = QString::from(key);
        qobject::mask_drop_frame(&k);
    }

    pub fn self_check(key: &str) -> String {
        let k = QString::from(key);
        qobject::mask_self_check(&k).to_string()
    }

    pub fn screens() -> Vec<Screen> {
        let count = qobject::pin_screen_count();
        (0..count)
            .map(|index| Screen {
                name: qobject::pin_screen_name(index).to_string(),
                x: qobject::pin_screen_x(index),
                y: qobject::pin_screen_y(index),
                w: qobject::pin_screen_w(index),
                h: qobject::pin_screen_h(index),
                dpr: qobject::pin_screen_device_pixel_ratio(index),
            })
            .collect()
    }
}
