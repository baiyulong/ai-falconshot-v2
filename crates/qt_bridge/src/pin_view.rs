//! One pin window's controller.
//!
//! `PinView` is the whole of the QML surface for a single pin: the state machine
//! in `falcon_core::pin` decides, this object translates, and QML binds. Every
//! gesture arrives in the coordinates QML actually has, gets turned into the
//! physical desktop pixels the state machine speaks, and comes back out as
//! qproperty writes - so the window never has to ask a question mid-gesture.
//!
//! Deliberately absent: anything that re-encodes a picture per frame of motion.
//! Dragging and wheel-zooming write position and size only; [`crate::state`]
//! pushes new pixels when the content key moves, which rotation and cropping do
//! and zoom does not.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        /// One pin on screen. QML creates this inside `PinWindow.qml` and binds
        /// every size, position and flag off it.
        #[qobject]
        #[qml_element]
        #[qproperty(i64, pin_id)]
        #[qproperty(bool, live)]
        #[qproperty(i32, pos_x)]
        #[qproperty(i32, pos_y)]
        #[qproperty(i32, size_w)]
        #[qproperty(i32, size_h)]
        #[qproperty(i32, zoom)]
        #[qproperty(i32, opacity)]
        #[qproperty(bool, smooth)]
        #[qproperty(bool, topmost)]
        #[qproperty(bool, click_through)]
        #[qproperty(i32, revision)]
        #[qproperty(QString, image_key)]
        type PinView = super::PinViewRust;

        /// Pull the state machine's current answer for `pin_id`. QML calls this
        /// when the id it was handed changes, and after every gesture below.
        #[qinvokable]
        fn reload(self: Pin<&mut Self>);

        /// §5.9.1 - the drag delta in physical pixels; the floor keeps a pin from
        /// being pushed off the screen.
        #[qinvokable]
        #[cxx_name = "dragMove"]
        fn drag_move(self: Pin<&mut Self>, dx: i32, dy: i32);

        /// §5.9.2 - `wx`/`wy` are the cursor inside the window, physical pixels,
        /// and the pixel under it is the one that stays put.
        #[qinvokable]
        #[cxx_name = "wheelZoom"]
        fn wheel_zoom(self: Pin<&mut Self>, up: bool, wx: i32, wy: i32);

        #[qinvokable]
        #[cxx_name = "applyZoom"]
        fn apply_zoom(self: Pin<&mut Self>, pct: i32);

        #[qinvokable]
        #[cxx_name = "zoomStep"]
        fn zoom_step(self: Pin<&mut Self>, up: bool);

        #[qinvokable]
        #[cxx_name = "resetZoom"]
        fn reset_zoom(self: Pin<&mut Self>);

        /// §5.10.4 "重置" - the way the pin is being looked at, back to the
        /// defaults. The picture itself is untouched.
        #[qinvokable]
        #[cxx_name = "resetView"]
        fn reset_view(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "rotateCw"]
        fn rotate_cw(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "rotateCcw"]
        fn rotate_ccw(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "flipH"]
        fn flip_h(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "flipV"]
        fn flip_v(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "toggleGrayscale"]
        fn toggle_grayscale(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "toggleInvert"]
        fn toggle_invert(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "applyOpacity"]
        fn apply_opacity(self: Pin<&mut Self>, pct: i32);

        #[qinvokable]
        #[cxx_name = "opacityStep"]
        fn opacity_step(self: Pin<&mut Self>, up: bool);

        #[qinvokable]
        #[cxx_name = "applyTopmost"]
        fn apply_topmost(self: Pin<&mut Self>, on: bool);

        /// §5.9.17 鼠标穿透.
        #[qinvokable]
        #[cxx_name = "applyClickThrough"]
        fn apply_click_through(self: Pin<&mut Self>, on: bool);

        #[qinvokable]
        #[cxx_name = "applySmooth"]
        fn apply_smooth(self: Pin<&mut Self>, on: bool);

        /// §5.9.10 - a second call gives the size back.
        #[qinvokable]
        #[cxx_name = "toggleThumbnail"]
        fn toggle_thumbnail(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "exitThumbnail"]
        fn exit_thumbnail(self: Pin<&mut Self>);

        /// §5.9.9 - a rectangle in desktop pixels, mapped back through zoom,
        /// thumbnail and rotation by the state machine.
        #[qinvokable]
        #[cxx_name = "cropWindow"]
        fn crop_window(self: Pin<&mut Self>, x: i32, y: i32, w: i32, h: i32) -> bool;

        /// §5.9.11 - the right-drag selection, in the same desktop pixels, but it
        /// narrows what the window shows without touching the picture.
        #[qinvokable]
        #[cxx_name = "freeThumbnailWindow"]
        fn free_thumbnail_window(self: Pin<&mut Self>, x: i32, y: i32, w: i32, h: i32) -> bool;

        #[qinvokable]
        #[cxx_name = "undoCrop"]
        fn undo_crop(self: Pin<&mut Self>) -> bool;

        /// §5.9.9 "恢复原图".
        #[qinvokable]
        #[cxx_name = "restoreOriginal"]
        fn restore_original(self: Pin<&mut Self>) -> bool;
    }
}

use core::pin::Pin;
use cxx_qt_lib::QString;

use falcon_core::geometry::{PhysPoint, PhysRect, PhysSize};
use falcon_core::pin::{PinId, THUMB_FIXED_DEFAULT};

use crate::state;

#[derive(Default)]
pub struct PinViewRust {
    pin_id: i64,
    live: bool,
    pos_x: i32,
    pos_y: i32,
    size_w: i32,
    size_h: i32,
    zoom: i32,
    opacity: i32,
    smooth: bool,
    topmost: bool,
    click_through: bool,
    revision: i32,
    image_key: QString,
}

impl qobject::PinView {
    /// Copy the state machine's answer into the properties QML binds to.
    pub fn reload(mut self: Pin<&mut Self>) {
        let id = *self.pin_id() as PinId;
        let view = state::with(|s| s.view_data(id));
        let key = format!("{id}-{}", view.revision);

        self.as_mut().set_live(view.live);
        self.as_mut().set_pos_x(view.pos_x);
        self.as_mut().set_pos_y(view.pos_y);
        self.as_mut().set_size_w(view.size_w);
        self.as_mut().set_size_h(view.size_h);
        self.as_mut().set_zoom(view.zoom);
        self.as_mut().set_opacity(view.opacity);
        self.as_mut().set_smooth(view.smooth);
        self.as_mut().set_topmost(view.topmost);
        self.as_mut().set_click_through(view.click_through);
        self.as_mut().set_revision(view.revision as i32);
        self.as_mut().set_image_key(QString::from(&*key));
    }

    /// One gesture done to the pin, then the properties brought back in line.
    /// Everything below is this one line, which is the point: the state machine
    /// decides, and this object only translates in and out.
    fn gesture<R: Default>(mut self: Pin<&mut Self>, f: impl FnOnce(&mut dyn Gesture) -> R) -> R {
        let id = *self.pin_id() as PinId;
        let done = state::with(|s| s.edit(id, |p, cfg, desk| f(&mut Ops { p, cfg, desk })))
            .unwrap_or_default();
        self.as_mut().reload();
        done
    }

    pub fn drag_move(self: Pin<&mut Self>, dx: i32, dy: i32) {
        self.gesture(|o| o.move_by(dx, dy));
    }

    pub fn wheel_zoom(self: Pin<&mut Self>, up: bool, wx: i32, wy: i32) {
        // The cursor arrives window-relative; the anchor the state machine wants
        // is a desktop point, because that is the space its window rect lives in.
        let at = PhysPoint::new(self.pos_x() + wx, self.pos_y() + wy);
        self.gesture(move |o| o.zoom_step(up, Some(at)));
    }

    pub fn apply_zoom(self: Pin<&mut Self>, pct: i32) {
        self.gesture(move |o| o.apply_zoom(pct.max(0) as u32));
    }

    pub fn zoom_step(self: Pin<&mut Self>, up: bool) {
        self.gesture(move |o| o.zoom_step(up, None));
    }

    pub fn reset_zoom(self: Pin<&mut Self>) {
        self.gesture(|o| o.reset_zoom());
    }

    pub fn reset_view(self: Pin<&mut Self>) {
        self.gesture(|o| o.reset_view());
    }

    pub fn rotate_cw(self: Pin<&mut Self>) {
        self.gesture(|o| o.rotate(1));
    }

    pub fn rotate_ccw(self: Pin<&mut Self>) {
        self.gesture(|o| o.rotate(-1));
    }

    pub fn flip_h(self: Pin<&mut Self>) {
        self.gesture(|o| o.flip_h());
    }

    pub fn flip_v(self: Pin<&mut Self>) {
        self.gesture(|o| o.flip_v());
    }

    pub fn toggle_grayscale(self: Pin<&mut Self>) {
        self.gesture(|o| o.grayscale());
    }

    pub fn toggle_invert(self: Pin<&mut Self>) {
        self.gesture(|o| o.invert());
    }

    pub fn apply_opacity(self: Pin<&mut Self>, pct: i32) {
        self.gesture(move |o| o.apply_opacity(pct.max(0) as u32));
    }

    pub fn opacity_step(self: Pin<&mut Self>, up: bool) {
        self.gesture(move |o| o.opacity_step(up));
    }

    pub fn apply_topmost(self: Pin<&mut Self>, on: bool) {
        self.gesture(move |o| o.apply_topmost(on));
    }

    pub fn apply_click_through(self: Pin<&mut Self>, on: bool) {
        self.gesture(move |o| o.apply_click_through(on));
    }

    pub fn apply_smooth(self: Pin<&mut Self>, on: bool) {
        self.gesture(move |o| o.apply_smooth(on));
    }

    pub fn toggle_thumbnail(self: Pin<&mut Self>) {
        self.gesture(|o| o.toggle_thumbnail(THUMB_FIXED_DEFAULT));
    }

    pub fn exit_thumbnail(self: Pin<&mut Self>) {
        self.gesture(|o| o.exit_thumbnail());
    }

    pub fn crop_window(self: Pin<&mut Self>, x: i32, y: i32, w: i32, h: i32) -> bool {
        let rect = PhysRect::new(x, y, w.max(0) as u32, h.max(0) as u32);
        self.gesture(move |o| o.crop_window(&rect))
    }

    /// §5.9.11 - the right-drag rectangle, in the same desktop space a crop's is.
    pub fn free_thumbnail_window(self: Pin<&mut Self>, x: i32, y: i32, w: i32, h: i32) -> bool {
        let rect = PhysRect::new(x, y, w.max(0) as u32, h.max(0) as u32);
        self.gesture(move |o| o.free_thumbnail_window(&rect))
    }

    pub fn undo_crop(self: Pin<&mut Self>) -> bool {
        self.gesture(|o| o.undo_crop())
    }

    /// §5.9.9 "恢复原图" - always succeeds, so the answer is whether anything moved.
    pub fn restore_original(self: Pin<&mut Self>) -> bool {
        self.gesture(|o| o.restore_original())
    }
}

/// The subset of [`falcon_core::pin::PinItem`] a gesture needs, spelled without
/// the config and desktop arguments so each invokable stays one line. A trait
/// rather than a bare closure because it lets a gesture answer with the value the
/// state machine returned - `Result` in, `bool` out.
trait Gesture {
    fn move_by(&mut self, dx: i32, dy: i32);
    fn zoom_step(&mut self, up: bool, at: Option<PhysPoint>) -> u32;
    fn apply_zoom(&mut self, pct: u32);
    fn reset_zoom(&mut self);
    fn reset_view(&mut self);
    fn rotate(&mut self, turns: i32);
    fn flip_h(&mut self);
    fn flip_v(&mut self);
    fn grayscale(&mut self);
    fn invert(&mut self);
    fn apply_opacity(&mut self, pct: u32);
    fn opacity_step(&mut self, up: bool);
    fn apply_topmost(&mut self, on: bool);
    fn apply_click_through(&mut self, on: bool);
    fn apply_smooth(&mut self, on: bool);
    fn toggle_thumbnail(&mut self, box_size: PhysSize);
    fn exit_thumbnail(&mut self);
    fn crop_window(&mut self, rect: &PhysRect) -> bool;
    fn free_thumbnail_window(&mut self, rect: &PhysRect) -> bool;
    fn undo_crop(&mut self) -> bool;
    fn restore_original(&mut self) -> bool;
}

struct Ops<'a> {
    p: &'a mut falcon_core::pin::PinItem,
    cfg: &'a falcon_core::config::Pin,
    desk: &'a falcon_core::pin::Desktop,
}

impl<'a> Gesture for Ops<'a> {
    fn move_by(&mut self, dx: i32, dy: i32) {
        self.p.move_by(dx, dy, self.desk);
    }
    fn zoom_step(&mut self, up: bool, at: Option<PhysPoint>) -> u32 {
        self.p.zoom_step(up, at)
    }
    fn apply_zoom(&mut self, pct: u32) {
        self.p.set_zoom_about(pct, None);
    }
    fn reset_zoom(&mut self) {
        self.p.reset_zoom();
    }
    fn reset_view(&mut self) {
        self.p.reset_view(self.cfg);
    }
    fn rotate(&mut self, turns: i32) {
        self.p.rotate(turns);
    }
    fn flip_h(&mut self) {
        self.p.toggle_flip_h();
    }
    fn flip_v(&mut self) {
        self.p.toggle_flip_v();
    }
    fn grayscale(&mut self) {
        self.p.toggle_grayscale();
    }
    fn invert(&mut self) {
        self.p.toggle_invert();
    }
    fn apply_opacity(&mut self, pct: u32) {
        self.p.set_opacity(pct);
    }
    fn opacity_step(&mut self, up: bool) {
        self.p.opacity_step(up);
    }
    fn apply_topmost(&mut self, on: bool) {
        self.p.set_topmost(on);
    }
    fn apply_click_through(&mut self, on: bool) {
        self.p.set_click_through(on);
    }
    fn apply_smooth(&mut self, on: bool) {
        self.p.set_smooth_zoom(on);
    }
    fn toggle_thumbnail(&mut self, box_size: PhysSize) {
        self.p.toggle_thumbnail(box_size);
    }
    fn exit_thumbnail(&mut self) {
        let _ = self.p.exit_thumbnail();
    }
    fn crop_window(&mut self, rect: &PhysRect) -> bool {
        self.p.crop_window(rect).is_ok()
    }
    fn free_thumbnail_window(&mut self, rect: &PhysRect) -> bool {
        self.p.free_thumbnail_window(rect).is_ok()
    }
    fn undo_crop(&mut self) -> bool {
        self.p.undo_crop().is_ok()
    }
    fn restore_original(&mut self) -> bool {
        self.p.restore_original()
    }
}
