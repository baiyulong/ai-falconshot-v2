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
        /// A selection exists, which is what makes the eight handles appear. Read
        /// off `hole.is_empty()` rather than from a drag flag, so a hole the harness
        /// set without any pointer at all gets the same treatment.
        #[qproperty(bool, selected)]
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
        /// The ink texture for this screen, empty while the layer has nothing to
        /// show. Its own key for the same reason `key` has one: a stroke that reused
        /// the previous stroke's URL would never be re-requested, so the second
        /// object would be in the document and nowhere on screen (§3.6 constraint 8).
        #[qproperty(QString, overlay_key)]
        /// The active tool's index into [`crate::annotate::TOOLS`]: 0 is the arrow,
        /// which is what gives the pointer back to the selection's eight grips.
        #[qproperty(i32, tool)]
        #[qproperty(i32, objects)]
        #[qproperty(bool, can_undo)]
        #[qproperty(bool, can_redo)]
        /// The active pen, read back rather than kept in the toolbar: §5.7.1's
        /// per-tool memory means a tool can come back with a width and a colour the
        /// buttons never set, and a toolbar that showed its own last click would then
        /// be lying about what draws the next stroke.
        #[qproperty(i32, pen_width)]
        /// `0xRRGGBB`. Packed because QML only needs it to light the matching chip and
        /// to seed the wheel's start, and three separate properties would be three
        /// more chances for a binding to read half a colour.
        #[qproperty(i32, pen_rgb)]
        #[qproperty(bool, dashed)]
        #[qproperty(bool, filled)]
        /// §5.7.13 step 5's three settings, and the flag that says whether the tool in
        /// hand is the one they belong to. The flag is Rust's answer rather than a QML
        /// comparison against a tool index, because the index of 放大 in
        /// [`crate::annotate::TOOLS`] is Rust's number and a second copy of it in QML is
        /// a second ladder that can disagree with the first.
        #[qproperty(bool, zoom_tool)]
        #[qproperty(i32, zoom_percent)]
        #[qproperty(bool, zoom_border)]
        #[qproperty(bool, connection_line)]
        /// §5.7.11's text field: whether one is open, and where. Rust sizes the box from
        /// the font leg and divides it into this window's DIP, so the field QML makes is
        /// the box the letters will be drawn in rather than a guess at it.
        #[qproperty(bool, text_editing)]
        #[qproperty(i32, text_x)]
        #[qproperty(i32, text_y)]
        #[qproperty(i32, text_w)]
        #[qproperty(i32, text_h)]
        /// 字号 in DIP, by the same division and for the same reason.
        #[qproperty(i32, text_font)]
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

        /// The pointer went down, in this window's own device-independent pixels.
        /// Returns the grip code (`0` none, `1` a new rectangle, `2..9` an edge or
        /// corner in `Handle::all()` order, `10` the whole rectangle), which QML uses
        /// for the cursor - the hit test lives in Rust, so the pointer and the
        /// picture cannot disagree about what is being held.
        #[qinvokable]
        #[cxx_name = "pressAt"]
        fn press(self: Pin<&mut Self>, x: f64, y: f64) -> i32;

        /// The same grip code for a point the pointer is only *over*. Hover cannot
        /// go through `pressAt`, which would take the grip; the two share one hit
        /// test in Rust so the cursor never promises a resize the press then does
        /// not perform.
        #[qinvokable]
        #[cxx_name = "hitTestAt"]
        fn hit_test(self: &Self, x: f64, y: f64) -> i32;

        /// The pointer moved while held.
        #[qinvokable]
        #[cxx_name = "dragTo"]
        fn drag(self: Pin<&mut Self>, x: f64, y: f64);
        /// The pointer let go.
        #[qinvokable]
        #[cxx_name = "releaseAt"]
        fn release(self: Pin<&mut Self>);

        /// Enter or a double-click: `true` when there was a selection to confirm.
        #[qinvokable]
        #[cxx_name = "commitHole"]
        fn commit(self: Pin<&mut Self>) -> bool;

        /// The ink finishers (§5.7.5's 折线, §5.7.13's 放大), which 双击 and `Enter`
        /// reach *before* `commitHole`: the same keystroke means three things to this
        /// mask, and `true` - "the press closed something of the layer's" - is what
        /// tells the caller the crop is not this one's. Which of the two was closed is
        /// [`crate::mask::MaskState::finish_ink`]'s answer, not this binding's.
        #[qinvokable]
        #[cxx_name = "finishInk"]
        fn finish_ink(self: Pin<&mut Self>) -> bool;

        /// Esc: `true` when it cleared a selection, `false` when the caller should
        /// cancel the capture. One keystroke must not both clear and cancel.
        #[qinvokable]
        #[cxx_name = "stepBack"]
        fn step_back(self: Pin<&mut Self>) -> bool;

        /// Arrow keys (§5.3.9): `edge` resizes the bottom-right instead of moving.
        #[qinvokable]
        #[cxx_name = "nudgeHole"]
        fn nudge(self: Pin<&mut Self>, dx: i32, dy: i32, edge: bool);

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

        /// §5.7.2's tool picker: the index into [`crate::annotate::TOOLS`]. Switching
        /// takes the pointer away from the selection's handles (or gives it back),
        /// so every window has to re-read the state - which is what the `grabbed`
        /// signal `main.qml` raises on this does.
        #[qinvokable]
        #[cxx_name = "selectTool"]
        fn select_tool(self: Pin<&mut Self>, code: i32);

        /// §5.7.2 step 2's pen colour, as three 0..255 channels. Opaque only, for
        /// now: the alpha the model carries is §5.7.16's job.
        #[qinvokable]
        #[cxx_name = "setColor"]
        fn set_color(self: Pin<&mut Self>, r: i32, g: i32, b: i32);

        /// §5.7.20's 多级画笔粗细, in device pixels. The brush diameter follows it.
        #[qinvokable]
        #[cxx_name = "setWidth"]
        fn set_width(self: Pin<&mut Self>, width: i32);

        /// §5.7.2 step 3's 虚线 and step 4's 填充.
        ///
        /// `apply_` for the same reason `apply_shader` needs it: `dashed` and
        /// `filled` are qproperties now, so `set_dashed`/`set_filled` already exist as
        /// their setters and cxx-qt refuses the bridge on a duplicate.
        #[qinvokable]
        #[cxx_name = "applyDashed"]
        fn apply_dashed(self: Pin<&mut Self>, on: bool);
        #[qinvokable]
        #[cxx_name = "applyFilled"]
        fn apply_filled(self: Pin<&mut Self>, on: bool);

        /// §5.7.13 step 5: 放大倍数、边框和连接线. `apply_` again, for the same
        /// duplicate-name reason as `apply_dashed`.
        #[qinvokable]
        #[cxx_name = "applyZoomPercent"]
        fn apply_zoom_percent(self: Pin<&mut Self>, percent: i32);
        #[qinvokable]
        #[cxx_name = "applyZoomBorder"]
        fn apply_zoom_border(self: Pin<&mut Self>, on: bool);
        #[qinvokable]
        #[cxx_name = "applyConnectionLine"]
        fn apply_connection_line(self: Pin<&mut Self>, on: bool);

        /// §5.7.11 step 3: the field's whole current string, on every change. The layer
        /// keeps the string and nothing else - no caret, no selection, no IME preedit -
        /// because the field owns those and a second copy in Rust would be a copy of an
        /// edit it never saw.
        #[qinvokable]
        #[cxx_name = "setText"]
        fn set_text(self: Pin<&mut Self>, text: &QString);

        /// §5.7.14's 撤销 / 重做 and §5.7.15's 全部清除. `true` is "there was something
        /// to do": the toolbar greys itself off from `can_undo`/`can_redo`, and a
        /// keystroke that hit an empty stack has to be able to say so.
        #[qinvokable]
        #[cxx_name = "undoStep"]
        fn undo_step(self: Pin<&mut Self>) -> bool;
        #[qinvokable]
        #[cxx_name = "redoStep"]
        fn redo_step(self: Pin<&mut Self>) -> bool;
        #[qinvokable]
        #[cxx_name = "clearInk"]
        fn clear_ink(self: Pin<&mut Self>) -> bool;

        /// The tool buttons' labels, in [`crate::annotate::TOOLS`]' order. Read once
        /// by the toolbar's `Component.onCompleted`: the list is Rust's, so a tool
        /// renamed in one place cannot be a different tool in the other.
        #[qinvokable]
        #[cxx_name = "toolNames"]
        fn tool_names(self: &Self) -> QString;
    }
}

use core::pin::Pin;
use cxx_qt_lib::QString;

use falcon_core::geometry::PhysRect;

use crate::annotate;
use crate::mask;

#[derive(Default)]
pub struct MaskViewRust {
    index: i32,
    live: bool,
    shown: bool,
    selected: bool,
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
    overlay_key: QString,
    tool: i32,
    objects: i32,
    can_undo: bool,
    can_redo: bool,
    pen_width: i32,
    pen_rgb: i32,
    dashed: bool,
    filled: bool,
    zoom_tool: bool,
    zoom_percent: i32,
    zoom_border: bool,
    connection_line: bool,
    text_editing: bool,
    text_x: i32,
    text_y: i32,
    text_w: i32,
    text_h: i32,
    text_font: i32,
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
    ///
    /// This is also the one place the ink is *published*. `flush` hands repainted
    /// canvases to Qt under a new key, and doing it here rather than inside the
    /// mutating call means the GUI thread is always the thread whose store QML is
    /// about to read from - and a window that reloads twice in a row gets the same
    /// key both times, because the second flush has nothing new to say.
    pub fn reload(mut self: Pin<&mut Self>) {
        let index = *self.index();
        let (view, shader, label, selected, ink) = mask::with(|m| {
            m.ink.flush();
            let label = m
                .slot(index.max(0) as usize)
                .map(|s| (s.key.clone(), s.name.clone()));
            let overlay = label
                .as_ref()
                .map(|(_, name)| m.ink.overlay_key(name))
                .unwrap_or_default();
            (
                m.view_data(index.max(0) as usize),
                m.shader,
                label,
                m.has_selection(),
                (
                    overlay,
                    m.ink.tool_code(),
                    m.ink.objects() as i32,
                    m.ink.can_undo(),
                    m.ink.can_redo(),
                    m.ink.width() as i32,
                    (i32::from(m.ink.color()[0]) << 16)
                        | (i32::from(m.ink.color()[1]) << 8)
                        | i32::from(m.ink.color()[2]),
                    m.ink.dashed(),
                    m.ink.filled(),
                    m.ink.zoom_tool(),
                    m.ink.zoom_percent(),
                    m.ink.zoom_border(),
                    m.ink.connection_line(),
                ),
            )
        });

        self.as_mut().set_live(view.live);
        self.as_mut().set_shown(view.shown);
        self.as_mut().set_selected(selected);
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
        let (
            overlay,
            tool,
            objects,
            can_undo,
            can_redo,
            width,
            rgb,
            dashed,
            filled,
            zoom_tool,
            zoom_percent,
            zoom_border,
            connection_line,
        ) = ink;
        self.as_mut().set_overlay_key(QString::from(&*overlay));
        self.as_mut().set_tool(tool);
        self.as_mut().set_objects(objects);
        self.as_mut().set_can_undo(can_undo);
        self.as_mut().set_can_redo(can_redo);
        self.as_mut().set_pen_width(width);
        self.as_mut().set_pen_rgb(rgb);
        self.as_mut().set_dashed(dashed);
        self.as_mut().set_filled(filled);
        self.as_mut().set_zoom_tool(zoom_tool);
        self.as_mut().set_zoom_percent(zoom_percent);
        self.as_mut().set_zoom_border(zoom_border);
        self.as_mut().set_connection_line(connection_line);
        self.as_mut().set_text_editing(view.typing_live);
        self.as_mut().set_text_x(view.typing.x);
        self.as_mut().set_text_y(view.typing.y);
        self.as_mut().set_text_w(view.typing.w as i32);
        self.as_mut().set_text_h(view.typing.h as i32);
        self.as_mut().set_text_font(view.font_dip as i32);
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

    /// The three pointer calls all end in `reload`, because the hole this window
    /// draws is a function of the *global* hole: a drag has to move the dim on the
    /// screen it is on before the button is let go.
    pub fn press(mut self: Pin<&mut Self>, x: f64, y: f64) -> i32 {
        let index = *self.index();
        let grip = mask::with(|m| {
            m.press(index.max(0) as usize, x, y);
            m.grip_code()
        });
        self.as_mut().reload();
        grip
    }

    pub fn hit_test(&self, x: f64, y: f64) -> i32 {
        let index = *self.index();
        mask::with(|m| m.hit_test(index.max(0) as usize, x, y))
    }

    pub fn drag(mut self: Pin<&mut Self>, x: f64, y: f64) {
        let index = *self.index();
        mask::with(|m| m.drag(index.max(0) as usize, x, y));
        self.as_mut().reload();
    }

    pub fn release(mut self: Pin<&mut Self>) {
        mask::with(|m| m.release());
        self.as_mut().reload();
    }

    pub fn commit(mut self: Pin<&mut Self>) -> bool {
        let hole = mask::with(|m| m.commit());
        self.as_mut().reload();
        hole.is_some()
    }

    pub fn finish_ink(mut self: Pin<&mut Self>) -> bool {
        let closed = mask::with(|m| m.finish_ink());
        self.as_mut().reload();
        closed
    }

    pub fn step_back(mut self: Pin<&mut Self>) -> bool {
        let cleared = mask::with(|m| m.escape());
        self.as_mut().reload();
        cleared
    }

    pub fn nudge(mut self: Pin<&mut Self>, dx: i32, dy: i32, edge: bool) {
        mask::with(|m| m.nudge_by(dx, dy, edge));
        self.as_mut().reload();
    }

    pub fn apply_shader(mut self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.set_shader(on));
        self.as_mut().reload();
    }

    /// The tool and style calls all end in `reload` for one reason beyond the counts:
    /// `reload` is what flushes the layer, so a repaint done by Rust reaches Qt's
    /// texture store on the GUI thread, in the same step the URL changes.
    pub fn select_tool(mut self: Pin<&mut Self>, code: i32) {
        mask::with(|m| m.ink.select_tool(code));
        self.as_mut().reload();
    }

    pub fn set_color(mut self: Pin<&mut Self>, r: i32, g: i32, b: i32) {
        let rgba = [
            r.clamp(0, 255) as u8,
            g.clamp(0, 255) as u8,
            b.clamp(0, 255) as u8,
            255,
        ];
        mask::with(|m| m.ink.set_color(rgba));
        self.as_mut().reload();
    }

    pub fn set_width(mut self: Pin<&mut Self>, width: i32) {
        mask::with(|m| m.ink.set_width(width.max(1) as u32));
        self.as_mut().reload();
    }

    pub fn apply_dashed(mut self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.ink.set_dashed(on));
        self.as_mut().reload();
    }

    pub fn apply_filled(mut self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.ink.set_filled(on));
        self.as_mut().reload();
    }

    /// `max(0)` rather than a clamp here: the range the knob and the copy agree on is
    /// [`crate::annotate`]'s answer, and a negative number from QML becoming 800% by way
    /// of `as u32` is the kind of arithmetic only a cast between signed and unsigned
    /// could invent.
    pub fn apply_zoom_percent(mut self: Pin<&mut Self>, percent: i32) {
        mask::with(|m| m.ink.set_zoom_percent(percent.max(0) as u32));
        self.as_mut().reload();
    }

    pub fn apply_zoom_border(mut self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.ink.set_zoom_border(on));
        self.as_mut().reload();
    }

    pub fn apply_connection_line(mut self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.ink.set_connection_line(on));
        self.as_mut().reload();
    }

    /// §5.7.11 step 3. The reload is not decoration: the box is measured from the
    /// string, so every keystroke changes its width, and a field that kept the width it
    /// was opened with would clip the line it is being typed into.
    pub fn set_text(mut self: Pin<&mut Self>, text: &QString) {
        let text = text.to_string();
        mask::with(|m| m.ink.type_text(&text));
        self.as_mut().reload();
    }

    pub fn undo_step(mut self: Pin<&mut Self>) -> bool {
        let done = mask::with(|m| m.ink.undo_step());
        self.as_mut().reload();
        done
    }

    pub fn redo_step(mut self: Pin<&mut Self>) -> bool {
        let done = mask::with(|m| m.ink.redo_step());
        self.as_mut().reload();
        done
    }

    pub fn clear_ink(mut self: Pin<&mut Self>) -> bool {
        let done = mask::with(|m| m.ink.clear_ink());
        self.as_mut().reload();
        done
    }

    pub fn tool_names(&self) -> QString {
        QString::from(&*annotate::tool_names())
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
