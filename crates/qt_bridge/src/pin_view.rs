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
        /// §5.9.5–§5.9.8 的显示状态五枚：角度与四枚开关，逐枚读的是 core 那一枚字段本体
        /// （见 [`crate::state::PinViewData::rotation`] 那封信）。为什么非要有这五枚：
        /// §5.9.6 的规则是「系统**更新显示状态**」、§5.9.7 与 §5.9.8 的规则是「再次执行恢复」，
        /// 而这两句在没有这五枚之前只有菜单上那六行的**按下**能读——按下之后窗上读不到「现在是
        /// 灰的」，也就读不到「不再是灰的」是同一次开关还是另一次操作。现在两边读的是同一格。
        ///
        /// 两枚翻转在这里叫 `flipped_*` 而不是照 core 写成 `flip_*`：`flip_h`/`flip_v` 在本
        /// `extern "RustQt"` 里已经是两枚 qinvokable 的 Rust 名，同一对象上再放一枚同名的属性读
        /// 函数，「按下它」与「它现在是几」就成了同一个符号。
        #[qproperty(i32, rotation)]
        #[qproperty(bool, flipped_h)]
        #[qproperty(bool, flipped_v)]
        #[qproperty(bool, grayscale)]
        #[qproperty(bool, inverted)]
        /// §5.5.8「刷新截图」的两枚菜单行该不该出现在这张贴图的右键里，读的就是这一枚。它是
        /// [`crate::state::PinState::refreshable`] 那一个条件的搬运（这枚贴图的来历里有没有
        /// 一枚**原截图坐标**），QML 那一侧不再自己判第二遍（#281）。跟 §5.9.15 的
        /// `has_transparent_pixels` 同一个形状：菜单行的**可见**与按下时的**落点**共用一格，
        /// 于是「看得见按不动」与「按得动看不见」这两种骗人的门在这一层就组不出来（#494）。
        #[qproperty(bool, refreshable)]
        /// §5.9.3、§5.9.4 与 §5.8.5/§5.8.6 那三扇**输入**对话框共用的那一格拒语。填进去的只有
        /// [`Self::apply_size_text`]、[`Self::opacity_from_text`] 与 [`Self::apply_card_edit`]
        /// 三只手，而串的本体是 [`falcon_core::pin::PinError`] 那六臂中文（`#[error(...)]` 那句）
        /// 与 [`crate::state::PinState::edit_card`] 那句拒绝，桥一个字也不另写
        /// （#281：一句话只有一个作者，见 `pin.rs` 里那段「对话框句子向来只有一个作者」）。
        /// 成功那一臂清空它，好让下一趟开框不会带着上一趟的错字。窗侧唯一读者是页上那行
        /// 提示，绑定 `dialog_message` 本体，不数、不判、不改写。
        #[qproperty(QString, dialog_message)]
        /// §5.8.5 步骤 4 / §5.8.6 步骤 4：这一枚**是不是**卡片贴出来的。菜单那一行
        /// 「卡片设置…」的 `visible` 读的就是这一格，而它的判断本体是 core 那一格
        /// [`falcon_core::pin::PinItem::card`] 有没有（经
        /// [`crate::state::PinViewData::is_card`] 搬过来，桥不判第二遍，#281）。形状与
        /// `refreshable` 同一式：「看得见、按下只会说这不是卡片」那一族门在这一层组不出来。
        #[qproperty(bool, is_card)]
        /// §5.8.6 步骤 4：这一枚是**颜色**卡片。页上读它只为一件事 —— 第四格那一个字段的
        /// 题头写「背景颜色」还是写「颜色值」；两种情况下填进去的都是
        /// [`falcon_core::colors::parse`] 读得进的那一串，拒与不拒也不由页上判。
        #[qproperty(bool, card_colour)]
        /// 那一格栅窗开起来带着的四枚读数：这一枚卡片**此刻**用的数，不是 `[card]` 的那四枚。
        /// 由 [`Self::read_card`] 现读，写手只有它一只（#281）。
        #[qproperty(i32, card_width)]
        #[qproperty(i32, card_font_size)]
        #[qproperty(QString, card_family)]
        #[qproperty(QString, card_background)]
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

        /// §5.9.3 步骤 1–4「打开贴图菜单 → 尺寸与缩放 → 输入宽度、高度或缩放百分比 →
        /// 按锁定比例更新结果」那一格在**这一枚窗**上的落点。三格文本原样搬进
        /// [`falcon_core::pin::PinItem::set_size_from_text`]：等比锁定、上下界、非法输入三判
        /// 全在 core 那一个作者手里（[`falcon_core::pin::size_zoom_request`]），桥只翻译。
        ///
        /// 交回的是**落在**窗上的那一枚缩放（core 的答案本体），`-1` 是被拒 —— 被拒时窗一动
        /// 不动（core 那一头判过），而为什么拒写在 [`dialog_message`] 那一格里，页上直接绑它。
        #[qinvokable]
        #[cxx_name = "applySizeText"]
        fn apply_size_text(
            self: Pin<&mut Self>,
            width: &QString,
            height: &QString,
            percent: &QString,
        ) -> i32;

        /// §5.9.4 步骤 2「或在菜单中输入透明度」那一格的**判据**门：把那一格文本交给
        /// [`falcon_core::pin::typed_opacity`]，交回判出来的百分比（`-1` = 拒，拒语写在
        /// [`dialog_message`]）。
        ///
        /// 它**不**改这枚贴图：透明度是 §5.10.3 的批量一手，「这一手改几枚」全仓只由
        /// [`crate::state::PinState::batch_hand`] 判一次，所以按下之后页上把这一枚数交给
        /// `Session.setPinOpacity`（与菜单那三行、Ctrl+滚轮同一只落点）。本窗不数名单，
        /// 也不给输入那一格另开第二条透明度路（#281）。
        #[qinvokable]
        #[cxx_name = "opacityFromText"]
        fn opacity_from_text(self: Pin<&mut Self>, raw: &QString) -> i32;

        /// §5.8.5 步骤 4 / §5.8.6 步骤 4 那一扇「卡片设置」的窗**开起来**时的读数：现读这一枚
        /// 卡片此刻用的那四枚数（`[card]` 打底、这一枚被调过的那几格盖上去 —— 那一路的作者是
        /// [`crate::state::PinState::card_view`]）。
        ///
        /// 为什么不在页上抄 `[card]` 当初值：打开 A 卡片的窗该看见 A 现在真的是多宽。页上显示
        /// 480 而 A 其实是 700 时，用户按 Ok 就是把 A 改回 480 —— 而那一手没人要求过。
        #[qinvokable]
        #[cxx_name = "readCard"]
        fn read_card(self: Pin<&mut Self>);

        /// §5.8.5 步骤 4 与 §5.8.6 步骤 4–5 那一四格文本的落点：原样交给
        /// [`crate::state::PinState::edit_card`]（空格 = 这一格没填 = 这一枚在这一格上仍跟着
        /// `[card]`，那一判由 [`crate::state::PinState::card_edit`] 一名作者判），排版、夹界、
        /// 颜色解析三手全在 core。
        ///
        /// `false` = 被拒：这一枚一处也不动（core 被拒时不提交身份），为什么拒写在
        /// [`dialog_message`] 那一格里，页上绑它。`true` = 改好了，新的那一张已经在窗上
        /// （本手自己 `reload` 一次，图幅可能变）。
        #[qinvokable]
        #[cxx_name = "applyCardEdit"]
        fn apply_card_edit(
            self: Pin<&mut Self>,
            width: &QString,
            font_size: &QString,
            font_family: &QString,
            background: &QString,
        ) -> bool;

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
use falcon_core::pin::{thumb_preset_box, PinId};

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
    rotation: i32,
    flipped_h: bool,
    flipped_v: bool,
    grayscale: bool,
    inverted: bool,
    refreshable: bool,
    is_card: bool,
    card_colour: bool,
    card_width: i32,
    card_font_size: i32,
    card_family: QString,
    card_background: QString,
    dialog_message: QString,
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
            rotation: 0,
            flipped_h: false,
            flipped_v: false,
            grayscale: false,
            inverted: false,
            refreshable: false,
            is_card: false,
            card_colour: false,
            card_width: 0,
            card_font_size: 0,
            card_family: QString::default(),
            card_background: QString::default(),
            dialog_message: QString::default(),
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
        self.as_mut().set_rotation(view.rotation);
        self.as_mut().set_flipped_h(view.flip_h);
        self.as_mut().set_flipped_v(view.flip_v);
        self.as_mut().set_grayscale(view.grayscale);
        self.as_mut().set_inverted(view.inverted);
        self.as_mut().set_refreshable(view.refreshable);
        self.as_mut().set_is_card(view.is_card);
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

    /// §5.9.3 那一扇「尺寸与缩放」对话框的落点。三格文本进 [`Gesture`] 之外的一条直路，
    /// 因为这一手要交回的是 core 的 `Result` 本体（`gesture` 那一格要求 `R: Default`，
    /// 而一枚「被拒 + 拒语」没有合法的 `Default` 形状可以塞进 `-1` 之外），形状照
    /// [`Self::pick_alpha_bg`] 那一封：`state::with` → 落一手 → `reload`。
    ///
    /// 被拒那一臂窗上**一处也不动**（core 在 [`falcon_core::pin::size_zoom_request`] 那一头
    /// 判过：拒，不钳、不替用户填没填的那一格），`reload` 因此只是把页上那三格初值取回真数。
    pub fn apply_size_text(
        mut self: Pin<&mut Self>,
        width: &QString,
        height: &QString,
        percent: &QString,
    ) -> i32 {
        let id = *self.pin_id() as PinId;
        let (w, h, p) = (width.to_string(), height.to_string(), percent.to_string());
        let answer = state::with(|s| {
            s.edit(id, |item, _, _| {
                item.set_size_from_text(&w, &h, &p)
            })
        });
        match answer {
            Some(Ok(zoom)) => {
                self.as_mut().set_dialog_message(QString::from(""));
                self.as_mut().reload();
                zoom as i32
            }
            Some(Err(why)) => {
                self.as_mut()
                    .set_dialog_message(QString::from(why.to_string().as_str()));
                self.as_mut().reload();
                -1
            }
            // 窗已经不在名单里（关掉了）：没有一句要替 core 编，页上那一格留原话。
            None => -1,
        }
    }

    /// §5.9.4 菜单输入那一格：只判，不动窗（理由见声明处那封信）。被拒时把 core 那句
    /// 拒语留在 [`dialog_message`]，页上绑它；被接受时清空并交回百分比，由页上交给
    /// `Session.setPinOpacity` 那只唯一的批量落点。
    pub fn opacity_from_text(mut self: Pin<&mut Self>, raw: &QString) -> i32 {
        match falcon_core::pin::typed_opacity(&raw.to_string()) {
            Ok(pct) => {
                self.as_mut().set_dialog_message(QString::from(""));
                pct as i32
            }
            Err(why) => {
                self.as_mut()
                    .set_dialog_message(QString::from(why.to_string().as_str()));
                -1
            }
        }
    }

    /// §5.8.5 步骤 4 那一扇窗**开起来**时的读数。它与 [`Self::reload`] 分开一只手，是因为这
    /// 四格只在窗打开的那一刻被读一次 —— 塞进 `reload` 就是让每一枚贴图的每一次滚轮都多搬
    /// 两枚字符串，而那笔钱没人要付。
    pub fn read_card(mut self: Pin<&mut Self>) {
        let id = *self.pin_id() as PinId;
        let card = state::with(|s| s.card_view(id));
        self.as_mut().set_card_colour(card.is_colour);
        self.as_mut().set_card_width(card.width as i32);
        self.as_mut().set_card_font_size(card.font_size as i32);
        self.as_mut()
            .set_card_family(QString::from(card.font_family.as_str()));
        self.as_mut()
            .set_card_background(QString::from(card.background.as_str()));
    }

    /// §5.8.5 步骤 4 / §5.8.6 步骤 4–5：四格文本进来，这一枚卡片自己重排。判据一处也不在
    /// 这里 —— 「空格 = 这一格没填」归 [`crate::state::PinState::card_edit`]，排版、夹界与
    /// 颜色解析归 core，被拒那一句也归它（[`crate::state::PinState::edit_card`]）。这一只手
    /// 只做两件事：把那句拒语留在 `dialog_message`（页上绑它），把改好的那一张搬回窗上。
    ///
    /// 成功那一路交回的是 `true` 而不是 core 那句读数：屏幕上重排好了的那一张、和下一次开框
    /// 读到的那四枚数（越界时被夹成的那一枚也在里面）就是这一手对用户的答复，而一句没人读的
    /// 话不许为了搬它再造第二格（#433 问的是「该说话的有没有说」，不是「每一句都得有个窗」）。
    pub fn apply_card_edit(
        mut self: Pin<&mut Self>,
        width: &QString,
        font_size: &QString,
        font_family: &QString,
        background: &QString,
    ) -> bool {
        let id = *self.pin_id() as PinId;
        let edit = state::PinState::card_edit(
            &width.to_string(),
            &font_size.to_string(),
            &font_family.to_string(),
            &background.to_string(),
        );
        match state::with(|s| s.edit_card(id, edit)) {
            Ok(_) => {
                self.as_mut().set_dialog_message(QString::from(""));
                self.as_mut().reload();
                true
            }
            Err(why) => {
                self.as_mut().set_dialog_message(QString::from(why.as_str()));
                false
            }
        }
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

    /// §5.9.10 steps 1–3. The box is the user's `[pin]` preset, read from the
    /// state snapshot here (the one reader that turns the config into a gesture
    /// argument) and handed to the state machine, which owns the fit. A second
    /// press gives the size back — the `false` that [`Self::gesture`] reloads on.
    pub fn toggle_thumbnail(self: Pin<&mut Self>) {
        let box_size = state::with(|s| thumb_preset_box(&s.cfg));
        self.gesture(move |o| o.toggle_thumbnail(box_size));
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
