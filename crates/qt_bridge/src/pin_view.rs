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
//!
//! The one exception is a GIF (§5.9.14): there the *content* is what moves, once
//! per frame rather than once per pixel of motion. The timer in `PinWindow.qml`
//! is what asks, and only while a clip is playing, so a window holding a still
//! never reaches the encoder.

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
        #[qproperty(f64, dpr)]
        #[qproperty(i32, zoom)]
        #[qproperty(i32, opacity)]
        #[qproperty(bool, smooth)]
        #[qproperty(bool, topmost)]
        #[qproperty(bool, click_through)]
        #[qproperty(i32, revision)]
        #[qproperty(QString, image_key)]
        /// §5.9.14: the four numbers a clip needs. `gif_frames` is 0 for a pin
        /// that is not a GIF, which is the one fact that lets the window's timer
        /// and its menu go quiet without a second question.
        #[qproperty(i32, gif_frames)]
        #[qproperty(i32, gif_frame)]
        #[qproperty(bool, gif_playing)]
        #[qproperty(i32, gif_speed)]
        /// §5.9.15: the row of the four the pin is on, and whether the picture has
        /// a hole for any of them to be about.
        #[qproperty(i32, alpha_bg)]
        #[qproperty(bool, has_transparent_pixels)]
        /// §5.10.1: the one list lives in the state machine; this is this pin's row
        /// in it, read out on [`reload`]. QML binds its selection ring to nothing
        /// else, so a click that changes the list has exactly one thing to follow.
        #[qproperty(bool, selected)]
        /// §5.11: this pin is out of the way *because of Solo*, not because a dialog
        /// is sitting on it - `live` folds those two reasons into one boolean, and a
        /// gauge row that asks "did 独显 put this window away, and did it bring it
        /// back" cannot answer it from `live` alone. Read straight off core's field
        /// in [`crate::state::PinState::view_data`], so the window never guesses.
        #[qproperty(bool, solo_hidden)]
        type PinView = super::PinViewRust;
        /// Pull the state machine's current answer for `pin_id`. QML calls this
        /// when the id it was handed changes, and after every gesture below.
        #[qinvokable]
        fn reload(self: Pin<&mut Self>);

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

        // §5.10.4 的「重置视图」不在这一列里了（P134）：它改的是**名单**，落点是
        // `Session.resetPin`，判据是 [`crate::state::PinState::batch_hand`] 那一枚。留着这一格
        // 就是第二条重置路而它又没有读者 —— 与上面那段信里被删掉的透明度那两格同一封信。

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

        /// §5.9.15 步骤 2/3 — the row of the menu the user picked, 0..3 in the order
        /// the PRD lists them (透明、伪透明、深色棋盘格、浅色棋盘格). Answers whether
        /// the pin took it. Named apart from the `alpha_bg` property's own setter,
        /// which this calls and which takes the same `i32`.
        #[qinvokable]
        #[cxx_name = "setAlphaBg"]
        fn pick_alpha_bg(self: Pin<&mut Self>, index: i32) -> bool;

        /// §5.9.15's click rule — whether the point under the cursor is one the pin
        /// answers to. `wx`/`wy` are the cursor inside the window in physical
        /// pixels, the same two [`Self::wheel_zoom`] is handed.
        #[qinvokable]
        #[cxx_name = "hitAt"]
        fn hit_at(self: Pin<&mut Self>, wx: i32, wy: i32) -> bool;

        /// §5.9.14: one pass of the window's timer. `delta_ms` is what the timer
        /// waited - the state machine decides whether that is a new frame, and
        /// answers whether the picture it ships actually moved.
        #[qinvokable]
        #[cxx_name = "gifTick"]
        fn gif_tick(self: Pin<&mut Self>, delta_ms: i32) -> bool;

        /// §5.9.14 上一帧/下一帧. `dir` is -1 or 1; stepping pauses playback.
        #[qinvokable]
        #[cxx_name = "gifStep"]
        fn gif_step(self: Pin<&mut Self>, dir: i32);

        /// §5.9.14 暂停/继续. Answers the state playback is now in.
        #[qinvokable]
        #[cxx_name = "gifToggle"]
        fn gif_toggle(self: Pin<&mut Self>) -> bool;

        /// §5.9.14 回到第一帧.
        #[qinvokable]
        #[cxx_name = "gifFirst"]
        fn gif_first(self: Pin<&mut Self>);

        /// §5.9.14 "调整播放速度" between the rungs of the ladder in
        /// [`falcon_core::pin::GIF_SPEED_STEPS`].
        #[qinvokable]
        #[cxx_name = "gifSpeedStep"]
        fn gif_speed_step(self: Pin<&mut Self>, up: bool);

        #[qinvokable]
        #[cxx_name = "gifApplySpeed"]
        fn gif_apply_speed(self: Pin<&mut Self>, pct: i32);
    }
}

use core::pin::Pin;
use cxx_qt_lib::QString;

use falcon_core::geometry::{PhysPoint, PhysRect, PhysSize};
use falcon_core::pin::{PinId, THUMB_FIXED_DEFAULT};

use crate::state;

pub struct PinViewRust {
    pin_id: i64,
    live: bool,
    pos_x: i32,
    pos_y: i32,
    size_w: i32,
    size_h: i32,
    dpr: f64,
    zoom: i32,
    opacity: i32,
    smooth: bool,
    topmost: bool,
    click_through: bool,
    revision: i32,
    image_key: QString,
    gif_frames: i32,
    gif_frame: i32,
    gif_playing: bool,
    gif_speed: i32,
    alpha_bg: i32,
    has_transparent_pixels: bool,
    selected: bool,
    solo_hidden: bool,
}

/// `dpr` is the one field that must not start at zero: QML divides the physical
/// size by it, so a window that has not been told its ratio yet would otherwise
/// ask for an infinite width for the moment before the first `reload()`.
impl Default for PinViewRust {
    fn default() -> Self {
        Self {
            pin_id: 0,
            live: false,
            pos_x: 0,
            pos_y: 0,
            size_w: 0,
            size_h: 0,
            dpr: 1.0,
            zoom: 0,
            opacity: 0,
            smooth: false,
            topmost: false,
            click_through: false,
            revision: 0,
            image_key: QString::default(),
            gif_frames: 0,
            gif_frame: 0,
            gif_playing: false,
            gif_speed: 0,
            alpha_bg: 0,
            has_transparent_pixels: false,
            selected: false,
            solo_hidden: false,
        }
    }
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
        self.as_mut().set_dpr(view.dpr);
        self.as_mut().set_zoom(view.zoom);
        self.as_mut().set_opacity(view.opacity);
        self.as_mut().set_smooth(view.smooth);
        self.as_mut().set_topmost(view.topmost);
        self.as_mut().set_click_through(view.click_through);
        self.as_mut().set_revision(view.revision as i32);
        self.as_mut().set_image_key(QString::from(&*key));
        self.as_mut().set_gif_frames(view.gif_frames);
        self.as_mut().set_gif_frame(view.gif_frame);
        self.as_mut().set_gif_playing(view.gif_playing);
        self.as_mut().set_gif_speed(view.gif_speed);
        self.as_mut().set_alpha_bg(view.alpha_bg);
        self.as_mut()
            .set_has_transparent_pixels(view.has_transparent_pixels);
        self.as_mut().set_selected(view.selected);
        self.as_mut().set_solo_hidden(view.solo_hidden);
    }

    /// One gesture done to the pin, then the properties brought back in line.
    /// Everything below is this one line, which is the point: the state machine
    /// decides, and this object only translates in and out.
    ///
    /// 拖动不在这一列里了（§5.10.2）：一次拖动搬的是**名单**，而 `edit` 是"一枚贴图上的
    /// 一手势"那一格 —— 让它去数名单，就会有两处地方各自决定"这一拖算几枚"（#281）。所以
    /// 落点在 `Session.dragPin`，判在 [`crate::state::PinState::drag_pin`]。删掉的那一格
    /// `desk` 是这件事的读者：这一列里再没有第二手要用桌面的 clamp。
    ///
    /// 透明度也不在这一列里了（§5.10.3）：一次调整改的也是**名单**，而「这一手算几枚」全仓只判
    /// 一次（[`crate::state::PinState::batch_hand`]）。两只落点是 `Session.setPinOpacity`（绝对值）
    /// 与 `Session.stepPinOpacity`（阶梯，Ctrl+滚轮与菜单那两枚都走它）。§5.10.4 的「重置」自
    /// P134 起同一枚判据走同一只落点（`Session.resetPin`），所以 `cfg` 在这一列里没有读者了 ——
    /// 它是上一段那封信里「删掉的那一格」的第二位。这一列里剩下的每一手仍
    /// 是"一枚贴图上的一个手势"，所以 `reload` 只重读自己那一扇；批量那三手重读的是名单里每一扇。
    fn gesture<R: Default>(mut self: Pin<&mut Self>, f: impl FnOnce(&mut dyn Gesture) -> R) -> R {
        let id = *self.pin_id() as PinId;
        let done = state::with(|s| s.edit(id, |p, _, _| f(&mut Ops { p }))).unwrap_or_default();
        self.as_mut().reload();
        done
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

    /// §5.9.14: the timer's call, and deliberately not [`Self::gesture`] - a tick
    /// that moved nothing must not re-read fourteen properties fifty times a
    /// second. The reload is what a moved frame earns, never the tick itself.
    pub fn gif_tick(mut self: Pin<&mut Self>, delta_ms: i32) -> bool {
        let id = *self.pin_id() as PinId;
        let moved = state::with(|s| s.tick_gif(id, delta_ms.max(0) as u32));
        if moved {
            self.as_mut().reload();
        }
        moved
    }

    pub fn gif_step(self: Pin<&mut Self>, dir: i32) {
        self.gesture(move |o| o.gif_step(dir));
    }

    pub fn gif_toggle(self: Pin<&mut Self>) -> bool {
        self.gesture(|o| o.gif_toggle())
    }

    pub fn gif_first(self: Pin<&mut Self>) {
        self.gesture(|o| o.gif_first());
    }

    pub fn gif_speed_step(self: Pin<&mut Self>, up: bool) {
        self.gesture(move |o| o.gif_speed_step(up));
    }

    pub fn gif_apply_speed(self: Pin<&mut Self>, pct: i32) {
        self.gesture(move |o| o.gif_apply_speed(pct.max(0) as u32));
    }

    /// §5.9.15 步骤 3: the row the menu carries. Reloaded whether or not the pin
    /// took it — a refused row still has to leave the checkmark on the mode the
    /// pin is actually in, not on the row that was clicked.
    pub fn pick_alpha_bg(mut self: Pin<&mut Self>, index: i32) -> bool {
        let id = *self.pin_id() as PinId;
        let took = state::with(|s| s.set_alpha_bg(id, index));
        self.as_mut().reload();
        took
    }

    /// §5.9.15's click rule, and deliberately not [`Self::gesture`]: a press the
    /// pin refuses must not re-read fourteen properties, and nothing about the
    /// picture changes by being asked. The cursor arrives in the window's own
    /// physical pixels because that is all QML has; the window's position is the
    /// state machine's, so the two are added here rather than in a binding.
    pub fn hit_at(self: Pin<&mut Self>, wx: i32, wy: i32) -> bool {
        let id = *self.pin_id() as PinId;
        let at = PhysPoint::new(*self.pos_x() + wx, *self.pos_y() + wy);
        state::with(|s| s.hit_at(id, at))
    }
}

/// The subset of [`falcon_core::pin::PinItem`] a gesture needs, spelled without
/// the config and desktop arguments so each invokable stays one line. A trait
/// rather than a bare closure because it lets a gesture answer with the value the
/// state machine returned - `Result` in, `bool` out.
trait Gesture {
    fn zoom_step(&mut self, up: bool, at: Option<PhysPoint>) -> u32;
    fn apply_zoom(&mut self, pct: u32);
    fn reset_zoom(&mut self);
    fn rotate(&mut self, turns: i32);
    fn flip_h(&mut self);
    fn flip_v(&mut self);
    fn grayscale(&mut self);
    fn invert(&mut self);
    fn apply_topmost(&mut self, on: bool);
    fn apply_click_through(&mut self, on: bool);
    fn apply_smooth(&mut self, on: bool);
    fn toggle_thumbnail(&mut self, box_size: PhysSize);
    fn exit_thumbnail(&mut self);
    fn crop_window(&mut self, rect: &PhysRect) -> bool;
    fn free_thumbnail_window(&mut self, rect: &PhysRect) -> bool;
    fn undo_crop(&mut self) -> bool;
    fn restore_original(&mut self) -> bool;
    fn gif_step(&mut self, dir: i32);
    fn gif_toggle(&mut self) -> bool;
    fn gif_first(&mut self);
    fn gif_speed_step(&mut self, up: bool);
    fn gif_apply_speed(&mut self, pct: u32);
}

struct Ops<'a> {
    p: &'a mut falcon_core::pin::PinItem,
}

impl<'a> Gesture for Ops<'a> {
    fn zoom_step(&mut self, up: bool, at: Option<PhysPoint>) -> u32 {
        self.p.zoom_step(up, at)
    }
    fn apply_zoom(&mut self, pct: u32) {
        self.p.set_zoom_about(pct, None);
    }
    fn reset_zoom(&mut self) {
        self.p.reset_zoom();
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
    /// §5.9.14: the five hands of the player, each answering through the pin's
    /// own `Option`. A pin with no clip has no player to act on, which is not an
    /// error - it is the same nothing a still window reports for every one of
    /// these menu items.
    fn gif_step(&mut self, dir: i32) {
        if let Some(play) = self.p.gif.as_mut() {
            play.step(dir);
        }
    }
    fn gif_toggle(&mut self) -> bool {
        match self.p.gif.as_mut() {
            Some(play) => play.toggle_play(),
            None => false,
        }
    }
    fn gif_first(&mut self) {
        if let Some(play) = self.p.gif.as_mut() {
            play.first_frame();
        }
    }
    fn gif_speed_step(&mut self, up: bool) {
        if let Some(play) = self.p.gif.as_mut() {
            play.speed_step(up);
        }
    }
    fn gif_apply_speed(&mut self, pct: u32) {
        if let Some(play) = self.p.gif.as_mut() {
            play.set_speed(pct);
        }
    }
}
