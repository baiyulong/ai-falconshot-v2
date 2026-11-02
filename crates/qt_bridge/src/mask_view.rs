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
        /// §5.4.5 —— 这一扇窗的底该不该是透明的。遮罩窗自己写着 `color: "black"`，暗化层画
        /// 在它上面；一张透明白板要把暗化归零而底仍是黑，屏幕上出现的就还是一张黑幕。两格
        /// 由 [`crate::mask::MaskState::board_dim`] / [`crate::mask::MaskState::board_clear`]
        /// 同一次锁里交出，QML 只绑不判。
        #[qproperty(bool, board_clear)]
        #[qproperty(bool, shader)]
        /// §5.3.5 —— 遮罩是否显示选区尺寸的一枚影子，从 [`crate::mask::MaskState::show_dimensions`]
        /// 经 [`crate::mask::MaskViewData::show_dimensions`] 抄到这里，再由 QML 用来 gate 一行文字。
        /// 单独放在 qproperty 而不是让 QML 直接读 `settings::show_dimensions()`，是因为 `open`
        /// 时取的这一刻属于那一次流程，而不是手指按下去的那一秒（同 [`crate::mask::MaskState::auto_copy`] 那一格）。
        #[qproperty(bool, show_dimensions)]
        /// §5.3.8 —— 哪一格比例芯片亮着，抄自 [`crate::mask::MaskViewData::ratio_index`]；
        /// `-1` 是"自定义那一格"，其原文在 [`Self::ratio_spec`]。QML 只绑不判，所以"按钮上
        /// 亮着 1:1"与"拖出来锁的是 1:1"是同一个主人说的。
        #[qproperty(i32, ratio_index)]
        /// 锁**真的**在不在（[`crate::mask::MaskViewData::ratio_live`]）。自定义那一格打到一半
        /// 时 [`Self::ratio_index`] 仍是 -1 而这一枚是 `false`：那一格要点灰而不是假装锁着。
        #[qproperty(bool, ratio_live)]
        /// 自定义那一格的原文，与上面两格同一次锁里交出（同 [`Self::landed`] 那一路的理由：
        /// `String` 进不了 `Copy` 的 [`crate::mask::MaskViewData`]，只能走这条元组）。
        #[qproperty(QString, ratio_spec)]
        /// §5.3.6 第一条规则的那枚锚点：打进来的宽高会**从哪一角**扩展，
        /// [`crate::mask::anchor_code`] 的序号，名字表在 [`crate::mask::SIZE_ANCHOR_NAMES`]。
        /// 它是只读的读数而不是这一轮的按钮 —— PRD 那句要的是"明确"，不是"可挑"，而把它做成
        /// 一格能挑的芯片就多长一把与拖拽来历打架的第二主人（谁在最后定角，谁说了算）。
        #[qproperty(i32, size_anchor)]
        /// §5.3.7 —— 选区尺寸**按当前单位**的读数（[`crate::mask::MaskViewData::size_w`]）。
        /// 与 [`Self::hole_w`] 分开两格是本轮的量点：那两格是窗内几何（跨屏选区被
        /// `local_dip` 裁进这一扇窗、除过这一扇窗的缩放），这两格是"这一块有多大"那一句
        /// 话，两扇窗上必须同字。页上那行 W×H 从此绑这两格，`hole_w`/`hole_h` 只留给画法。
        #[qproperty(i32, size_w)]
        #[qproperty(i32, size_h)]
        /// 这一刻用的单位，[`crate::mask::unit_code`] 的序号；名字表在
        /// [`crate::mask::SIZE_UNITS`]，过桥走 [`Self::unitNames`] —— 与
        /// [`Self::size_anchor`] + [`Self::sizeAnchorNames`] 同一形状：**序号过桥、句子留在
        /// Rust**。页拿它做两件事：取下标亮那枚芯片，以及决定打字进去的数被解释成什么。
        #[qproperty(i32, size_unit)]
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
        /// §5.7.11 step 4's four knobs, and the flag that says whether the tool in hand
        /// is the one they belong to - the same shape `zoom_tool` gives 放大's row.
        /// `pen_font` is the *pen's* size in device pixels and `text_font` above is the
        /// open field's in DIP: the toolbar has to light the cell whose number the click
        /// sends, and the field has to be sized in the units its window draws in.
        #[qproperty(bool, text_tool)]
        #[qproperty(i32, pen_font)]
        #[qproperty(i32, text_align)]
        #[qproperty(bool, text_bg)]
        #[qproperty(bool, text_outline)]
        /// §5.7.11 step 4's 字体, read back rather than set: this round's control is the
        /// config file's row, and a row that reaches the pen without reaching the toolbar
        /// is a setting the user can only check by drawing something.
        #[qproperty(QString, pen_family)]
        /// §5.7.14's 擦除到透明, and the flag that says whether the tool in hand is the
        /// eraser it belongs to - the third row of the shape `zoom_tool` and `text_tool`
        /// give the two before it. The flag is Rust's because the mode is a pen field
        /// like any other, and §5.7.1's per-tool memory can hand the eraser back a mode
        /// the buttons have never clicked.
        #[qproperty(bool, eraser_tool)]
        #[qproperty(bool, erase_transparent)]
        /// §5.5.2's answer to the last 复制: the pixel count when the picture got
        /// through, and who was holding the clipboard when it did not. Rust owns the
        /// sentence (`MaskState::message`) because the writing owns it, and it arrives
        /// through `reload` like everything else here, so a toolbar that shows this is
        /// showing the same moment it shows the hole.
        #[qproperty(QString, message)]
        /// §5.5.4 step 4 的那条通知要能被**点**，所以路径本身得过界。`message` 是句
        /// 话，话里那个路径是让门从字里抠出来的（`landed_path`），一个字改一行就断；
        /// 这一条是 Rust 手里那个 `PathBuf` 的原文，点击走它，门也读它来对盘。空串是
        /// "这一张还没有落到任何地方"。
        #[qproperty(QString, landed)]
        /// 最近一次揭示的话，与 `message` 分着放：一句反馈不许把另一句证据挤掉。
        #[qproperty(QString, reveal_last)]
        /// 写之前那一档提醒（PRD 447 的"将被填充"）。它和 `message` 分着，正是因为
        /// 一个槽位里"将被"会被"已保存"顶掉——那就不算在写之前说过。空串 = 这一档不丢透明。
        #[qproperty(QString, pre_notice)]
        /// §5.14.1 那一行的话，与上面三条分着放：`record_history` 的五格分岔（拼不出图 /
        /// `enabled` 是关的 / 打不开 `history.db` / 那一行没写进去 / 写进去而 §5.14.4 的
        /// 清理没跑成）各说各的话，合并成一句就输掉"落盘成了而历史没记"这一整格证据。
        /// P93 只有写侧（`MaskState::history_last` 全仓零读者），这一条是它的第一位读者。
        /// 空串 = 这一场还没走到那道门。
        #[qproperty(QString, history_last)]
        /// §5.5.10 的那一句发送到应用的话，与上面几条分着放：派发有**四种**答（递出去了 /
        /// 目标不在这台机器上 / 路径拼不出一条安全参数 / shell 回了失败），合成一句就说不出
        /// 「点了两次、一次也没递到」与「递到了而对面不在这个机器上」的区别。空串 = 这一场
        /// 还没按过那一枚芯片。
        #[qproperty(QString, send_to_last)]
        /// 点击与揭示的两个计数，给这条腿在它自己前后各读一次用：一次点击记一次，
        /// 与开没开成窗无关；开了几个窗是另一个数。两个数并成一个就说不出"点了两次、
        /// 一次也没到 shell"这句话——而那正是默认设置下唯一该发生的事。
        #[qproperty(i32, reveal_clicks)]
        #[qproperty(i32, reveal_acted)]
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

        /// The pointer moved while held. `unlocked` is §5.3.8 的那句"用户应可以临时解除比例约束"
        /// —— QML 把 `mouse.modifiers` 里那一枚修饰键按下去的那一刻递进来，这一段拖拽就走自由
        /// 比例；它不进 [`Self::ratio_index`]，所以没有一把"临时按着却永久留在解除态"的门。
        #[qinvokable]
        #[cxx_name = "dragTo"]
        fn drag(self: Pin<&mut Self>, x: f64, y: f64, unlocked: bool);
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

        /// §5.7.11 step 4: 字号、对齐、背景 and 描边 - the four cells the row offers while
        /// `text_tool` is lit. `apply_` on the last two for the duplicate-name reason
        /// `apply_dashed` gives: `text_bg` and `text_outline` are qproperties now.
        #[qinvokable]
        #[cxx_name = "setFontSize"]
        fn set_font_size(self: Pin<&mut Self>, size: i32);
        #[qinvokable]
        #[cxx_name = "setAlign"]
        fn set_align(self: Pin<&mut Self>, code: i32);
        #[qinvokable]
        #[cxx_name = "applyTextBg"]
        fn apply_text_bg(self: Pin<&mut Self>, on: bool);
        #[qinvokable]
        #[cxx_name = "applyTextOutline"]
        fn apply_text_outline(self: Pin<&mut Self>, on: bool);

        /// §5.7.14's 擦除到透明. `apply_` on this one for the duplicate-name reason
        /// `apply_text_bg` gives: `erase_transparent` is a qproperty now, so
        /// `set_erase_transparent` is already its setter's name.
        #[qinvokable]
        #[cxx_name = "applyEraseTransparent"]
        fn apply_erase_transparent(self: Pin<&mut Self>, on: bool);

        /// §5.7.11 step 3: the field's whole current string, on every change. The layer
        /// keeps the string and nothing else - no caret, no selection, no IME preedit -
        /// because the field owns those and a second copy in Rust would be a copy of an
        /// edit it never saw.
        #[qinvokable]
        #[cxx_name = "setText"]
        fn set_text(self: Pin<&mut Self>, text: &QString);

        /// §5.7.19's 撤销 / 重做, and 全清 - the toolbar's own name for dropping the whole
        /// document, which no §5.7 section owns: §5.7.1 step 5's 删除 is per object, and
        /// that is §5.7.17's selection. `true` is "there was something to do": the toolbar
        /// greys itself off from `can_undo`/`can_redo`, and a keystroke that hit an empty
        /// stack has to be able to say so.
        #[qinvokable]
        #[cxx_name = "undoStep"]
        fn undo_step(self: Pin<&mut Self>) -> bool;
        #[qinvokable]
        #[cxx_name = "redoStep"]
        fn redo_step(self: Pin<&mut Self>) -> bool;
        #[qinvokable]
        #[cxx_name = "clearInk"]
        fn clear_ink(self: Pin<&mut Self>) -> bool;

        /// §5.5.2's 复制, from the toolbar's cell or from `Ctrl+C`: the selection with
        /// its ink onto the clipboard. `true` is "it is on the clipboard", and either
        /// way [`crate::mask::MaskState::copy_out`] has left a sentence in `message`
        /// for the toolbar to show - which is also why this does not close the mask.
        #[qinvokable]
        #[cxx_name = "copySelection"]
        fn copy_selection(self: Pin<&mut Self>) -> bool;

        /// §5.5.5's 另存为: the same stitched frame 复制 puts on the clipboard, written
        /// to the path the file dialog chose. `filter` is the extension text of the
        /// format the dialog was showing (`png`, `jpg`), which decides a name typed
        /// without one; an empty text is the dialog saying it offered nothing. `true` is
        /// "the file is there", and as with [`crate::mask::MaskState::copy_out`] the
        /// sentence - including the one about transparency the format cannot hold - is in
        /// `message` either way, so this neither closes the mask nor needs a second
        /// answer in QML.
        #[qinvokable]
        #[cxx_name = "saveSelection"]
        fn save_selection(self: Pin<&mut Self>, path: &QString, filter: &QString) -> bool;

        /// PRD 433 / 459's 贴图 after 保存: the same write as
        /// [`crate::mask::MaskState::save_out`], then a pin made from the file that
        /// landed. `true` is "the file is there *and* it is floating"; the one case
        /// where those disagree is named in `message` rather than collapsed into a
        /// bool, so QML shows the sentence and decides nothing about the picture.
        #[qinvokable]
        #[cxx_name = "saveAndPin"]
        fn save_and_pin(self: Pin<&mut Self>, path: &QString, filter: &QString) -> bool;

        /// §5.5.4's 快速保存: the same write 另存为 does, with no dialog and no name from
        /// the user - `[output]` decides where it goes and what it is called. `true` is
        /// "the file is there", and as with the two doors above the sentence is already in
        /// `message` either way, including the one about the directory the machine could
        /// not name and the one about a taken name under `collision = keep_both`.
        #[qinvokable]
        #[cxx_name = "quickSave"]
        fn quick_save(self: Pin<&mut Self>) -> bool;

        /// §5.5.4 step 4：通知上那一次点击。`true` 是 shell 收下了这条请求，而话——
        /// 包括"这张图还没有落到任何地方"和"要揭示的东西不在盘上"那两句拒绝——在
        /// [`crate::mask::MaskState::reveal_last`]，和 复制/保存 一样不经返回值传话。
        #[qinvokable]
        #[cxx_name = "revealLanded"]
        fn reveal_landed(self: Pin<&mut Self>) -> bool;

        /// The tool buttons' labels, in [`crate::annotate::TOOLS`]' order. Read once
        /// by the toolbar's `Component.onCompleted`: the list is Rust's, so a tool
        /// renamed in one place cannot be a different tool in the other.
        #[qinvokable]
        #[cxx_name = "toolNames"]
        fn tool_names(self: &Self) -> QString;

        /// §5.21.2 第一枚：同一排钮的悬停句，与 [`Self::toolNames`] 同枚数同序 ——
        /// 两个串都出自 [`crate::annotate`] 的同一张表，所以第 `i` 句提示只能落在
        /// 第 `i` 枚钮上。QML 侧 `.split("|")` 后按 index 递。
        #[qinvokable]
        #[cxx_name = "toolHints"]
        fn tool_hints(self: &Self) -> QString;

        /// §5.3.8 第一、二步：比例那一排的标签，与 [`Self::ratioHints`] 同枚数同序 —— 两个串
        /// 都出自 [`crate::mask::RATIO_CELLS`] 同一张表，所以第 `i` 句提示只能落在第 `i` 枚芯片上
        /// （与 [`Self::toolNames`] / [`Self::toolHints`] 同一条口径）。
        #[qinvokable]
        #[cxx_name = "ratioNames"]
        fn ratio_names(self: &Self) -> QString;

        #[qinvokable]
        #[cxx_name = "ratioHints"]
        fn ratio_hints(self: &Self) -> QString;

        /// §5.3.8 第一步：按下第 `index` 格芯片。返回 `false` 是"那一格不存在"，而锁**原样
        /// 不动** —— 返回值是这一枚能把"没改动"说出口的唯一通道。
        #[qinvokable]
        #[cxx_name = "pickRatio"]
        fn pick_ratio(self: Pin<&mut Self>, index: i32) -> bool;

        /// §5.3.8 第二步的后半：「或输入自定义比例」。那句话**总是**被记下（屏幕上显示的就是
        /// 用户打的字），返回值只说锁上没上 —— `false` 之后 [`Self::ratio_live`] 也是 `false`，
        /// 文本框点灰绑的就是这一枚读数（与 [`Self::ratio_index`] 同一次锁里交出的那三格）。
        ///
        /// `apply_` 而不是 `set_`：[`Self::ratio_spec`] 那枚 qproperty 已经生成了 associated
        /// function `set_ratio_spec`（与 [`Self::apply_shader`] 同一条理由）。
        #[qinvokable]
        #[cxx_name = "applyRatioSpec"]
        fn apply_ratio_spec(self: Pin<&mut Self>, spec: &QString) -> bool;

        /// §5.3.6 第三、四步：把 W 与 H 两格里打进来的数字换成选区尺寸，从
        /// [`Self::size_anchor`] 那一角扩展，超出桌面的那一头让路。
        ///
        /// 返回 `false` = **选区一点没动**（还没拖出选区 / 那对数不是正数 / 装不进 `u32`），
        /// 与 [`Self::pickRatio`] 的 `false` 同一条口径：门把手不许在"没做成"的时候顺手清掉
        /// 别的格子。返回 `true` 也不等于"这就是你打的数"—— 夹过的会变小，那是第二条规则。
        ///
        /// `apply_` 而不是 `set_`：与 [`Self::applyRatioSpec`]、[`Self::applyShader`] 同一条
        /// 理由，`set_` 那一族名字已经被 qproperty 的 associated function 占了。
        #[qinvokable]
        #[cxx_name = "applySize"]
        fn apply_size(self: Pin<&mut Self>, w: i32, h: i32) -> bool;

        /// §5.3.6 第一句规则在屏幕上的那行字：锚点的名字，`|` 分隔，下标是 [`Self::sizeAnchor`]
        /// 那一枚序号 —— 与 [`Self::ratioNames`] 同一形状，句子住在 Rust。
        #[qinvokable]
        #[cxx_name = "sizeAnchorNames"]
        fn size_anchor_names(self: &Self) -> QString;

        /// §5.3.7 第一步「用户在…**尺寸输入区域**选择单位」：单位字样表，`|` 分隔，下标是
        /// [`Self::sizeUnit`] —— 与 [`Self::sizeAnchorNames`] 同一形状，句子住在 Rust
        /// （[`crate::mask::unit_names`]，其本身又只是 [`crate::mask::SIZE_UNITS`] 里那两枚
        /// 单位的 `Unit::label`，所以字样没有第二个主人）。
        #[qinvokable]
        #[cxx_name = "unitNames"]
        fn unit_names(self: &Self) -> QString;

        /// §5.3.7 那两枚芯片的悬停句，`|` 分隔、与 [`Self::unitNames`] 同序（[`crate::mask::UNIT_HINTS`]）。
        /// 与 [`Self::ratioHints`] 同一分工：中文短语的翻译权不在这一页上。
        #[qinvokable]
        #[cxx_name = "unitHints"]
        fn unit_hints(self: &Self) -> QString;

        /// §5.3.7 第二步：把单位定成第 `code` 格（Repeater 的那一枚 —— 芯片上第 i 个字样只能
        /// 是清单里第 i 格，所以过界那一臂在 Rust 里答话，[`crate::mask::unit_at_code`]）。
        /// 返回**真正生效**的那一格的序号而不是 `bool`：这一格没有"没做成"的臂，而页上的芯片
        /// 必须跟着 Rust 的答案点亮、不能跟着自己递出去的那个下标点。
        #[qinvokable]
        #[cxx_name = "setUnit"]
        fn set_unit(self: Pin<&mut Self>, code: i32) -> i32;

        /// P91：§5.5.12 send-to 显示条目第一次跨过 Rust→QML 边界 —— 从
        /// [`crate::settings::output_cfg`] 走 [`crate::settings::pipeline_send_to`]、
        /// [`crate::settings::send_to_menu_entries`] 映射 UI 显示串，最后
        /// [`crate::settings::send_to_menu_names_joined`] 用 `|` 联成一枚纯串。QML
        /// 层 `.split("|")` 就能喂 Repeater；空集合 ⇒ 空串。真 Repeater 接线留下一轮。
        #[qinvokable]
        #[cxx_name = "sendToMenuNames"]
        fn send_to_menu_names(self: &Self) -> QString;

        /// P92：§5.5.11 share 显示条目第一次跨过 Rust→QML 边界 —— 从
        /// [`crate::settings::output_cfg`] 走 [`crate::settings::pipeline_share_targets`]、
        /// [`crate::settings::share_menu_entries`] 映射 UI 显示串，最后
        /// [`crate::settings::share_menu_names_joined`] 用 `|` 联成一枚纯串。这一半与
        /// P91 的 `sendToMenuNames` 对称。QML 层 `.split("|")` 就能喂 Repeater；空集合
        /// ⇒ 空串。真 Repeater 接线与 provider 派发留下一轮。
        #[qinvokable]
        #[cxx_name = "shareMenuNames"]
        fn share_menu_names(self: &Self) -> QString;

        /// P181：§5.7.21 那一排颜色芯片的名单第一次跨过这道桥 —— 从
        /// [`crate::settings::annotation_cfg`] 取档里 `[annotation] palette` 那一排，
        /// [`crate::settings::band_colors_joined`] 用档自己那台解析器（`colors::parse`）
        /// 归一成 `RRGGBB` 后 `|` 联起来。与 [`Self::sendToMenuNames`] 同一形状：空表 ⇒
        /// 空串，QML 侧 `.split("|")` 后先滤零长段再喂 Repeater。
        ///
        /// 本轮之前这一枚键在产品路径上**零读者**：`AnnotationToolbar.qml` 写着六枚自己挑
        /// 的字面量，而档里是十枚（`config.rs` 的 `default_palette`），改档里的颜色不动屏幕。
        #[qinvokable]
        #[cxx_name = "bandColors"]
        fn band_colors(self: &Self) -> QString;

        /// §5.5.10 步骤 2 的那一枚芯片被按下：把这一次的图交给第 `index` 枚目标应用。
        /// `index` 就是 Repeater 的那一枚 —— 工具条上第 i 枚芯片只能是清单里第 i 个目标，
        /// 所以过界的那一臂在 Rust 里答话（[`crate::mask::MaskState::send_to_target`]），
        /// 而不是 QML 自己再数一遍清单（数两遍就是两条会各自漂移的清单）。
        /// `true` 是 shell 收下了这一次请求；话——包括"目标不在这台机器上，请重新设置路径"
        /// 那一句拒绝——在 [`crate::mask::MaskState::send_to_last`]，和 复制/保存 一样不经
        /// 返回值传话。
        #[qinvokable]
        #[cxx_name = "sendToTarget"]
        fn send_to_target(self: Pin<&mut Self>, index: i32) -> bool;
    }
}

use core::pin::Pin;
use cxx_qt_lib::QString;

use falcon_core::geometry::PhysRect;

use crate::annotate;
use crate::mask;
use crate::settings;

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
    board_clear: bool,
    shader: bool,
    show_dimensions: bool,
    // §5.3.8 的三格。`Default` 给的 0/false/空串就是「无」，所以一张还没被挑过比例的窗
    // 与 P161 之前一模一样：锁关着，第一拖是自由矩形。
    ratio_index: i32,
    ratio_live: bool,
    ratio_spec: QString,
    // §5.3.6 的锚点。`Default` 给的 0 就是左上角，与 `MaskState::default()`、
    // `MaskViewData::default()` 那两枚是同一个答案，所以一张还没被分配槽位的窗不会报出
    // 一个越界下标去索引 `SIZE_ANCHOR_NAMES`。
    size_anchor: i32,
    // §5.3.7 的三格。`Default` 给的 0/0/0 与上面那一格同理：还没分配槽位的窗报的是 px、
    // 尺寸 0×0，而页上那两行都在 `selected` 的门后面，读不到这三格。
    size_w: i32,
    size_h: i32,
    size_unit: i32,
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
    text_tool: bool,
    pen_font: i32,
    text_align: i32,
    text_bg: bool,
    text_outline: bool,
    pen_family: QString,
    eraser_tool: bool,
    erase_transparent: bool,
    message: QString,
    landed: QString,
    reveal_last: QString,
    pre_notice: QString,
    send_to_last: QString,
    history_last: QString,
    reveal_clicks: i32,
    reveal_acted: i32,
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
        let (
            view,
            shader,
            label,
            selected,
            ink,
            message,
            landed,
            reveal_last,
            pre_notice,
            history_last,
            send_to_last,
            ratio_spec,
            clicks,
            acted,
            dim,
            clear,
        ) = mask::with(|m| {
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
                    m.ink.text_tool(),
                    m.ink.font_px() as i32,
                    m.ink.align_code(),
                    m.ink.text_bg_on(),
                    m.ink.text_outline_on(),
                    m.ink.font_family().to_string(),
                    m.ink.eraser_tool(),
                    m.ink.erase_transparent(),
                ),
                m.message.clone(),
                m.landed
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                m.reveal_last.clone(),
                m.pre_notice.clone(),
                m.history_last.clone(),
                m.send_to_last.clone(),
                m.ratio_spec.clone(),
                m.reveal_clicks as i32,
                m.reveal_acted as i32,
                // 这一块板子要暗化多少、窗底要不要跟着透明 —— 与上面那一样本同一次锁，因为
                // 它们是同一趟流程的读数：分两次问就可能出现"洞是新那一身的、暗化是上一张的"。
                m.board_dim(),
                m.board_clear(),
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
        // A see-through board is the one flow that draws 0 here ([`mask::MaskState::board_dim`]),
        // and `--mask` never reaches that arm: it opens a freeze, not a board.
        self.as_mut().set_dim_alpha(dim);
        self.as_mut().set_board_clear(clear);
        // 一块透明白板走四条暗化 `Rectangle` 那一路而不是 `ShaderEffect`：那一趟乘的是
        // `1 - dim_alpha`，dim 归零之后它要 exactly 报"什么都不压"才看得出桌面，而软件 RHI
        // 上 `status` 停在 Uncompiled 的这一族（P14 测过）已经证明"这一趟到底画了什么"不能
        // 靠猜。四矩形那一路在 dim=0 时是四张不画的东西，这是它能被读的那一种。
        self.as_mut().set_shader(shader && !clear);
        self.as_mut().set_show_dimensions(view.show_dimensions);
        // §5.3.8：芯片亮哪格、锁在不在、自定义那句话是什么 —— 三格与上面那一样本同一次锁，
        // 分两次问就可能出现"芯片写着 1:1、锁其实还留着上一场的 16:9"。
        self.as_mut().set_ratio_index(view.ratio_index);
        self.as_mut().set_ratio_live(view.ratio_live);
        self.as_mut().set_ratio_spec(QString::from(&*ratio_spec));
        // §5.3.6 那行"从哪一角扩展"就在这一趟里读出：`view` 与上面两格是同一次锁的样本，
        // 所以不会出现"尺寸是这一趟的、锚点是上一趟的"。
        self.as_mut().set_size_anchor(view.size_anchor);
        // §5.3.7 那对读数与那枚单位用的是**同一次锁里**的同一份样本（`view`），所以不会出现
        // "数字是这一趟的、单位是上一趟的"那一族；页上两行 W×H 从此绑这两格，`hole_w`/`hole_h`
        // 退回去只画暗化与八枚把手。
        self.as_mut().set_size_w(view.size_w as i32);
        self.as_mut().set_size_h(view.size_h as i32);
        self.as_mut().set_size_unit(view.size_unit);
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
            text_tool,
            pen_font,
            text_align,
            text_bg,
            text_outline,
            pen_family,
            eraser_tool,
            erase_transparent,
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
        self.as_mut().set_text_tool(text_tool);
        self.as_mut().set_pen_font(pen_font);
        self.as_mut().set_text_align(text_align);
        self.as_mut().set_text_bg(text_bg);
        self.as_mut().set_text_outline(text_outline);
        self.as_mut().set_pen_family(QString::from(&*pen_family));
        self.as_mut().set_eraser_tool(eraser_tool);
        self.as_mut().set_erase_transparent(erase_transparent);
        self.as_mut().set_text_editing(view.typing_live);
        self.as_mut().set_text_x(view.typing.x);
        self.as_mut().set_text_y(view.typing.y);
        self.as_mut().set_text_w(view.typing.w as i32);
        self.as_mut().set_text_h(view.typing.h as i32);
        self.as_mut().set_text_font(view.font_dip as i32);
        self.as_mut().set_message(QString::from(&*message));
        self.as_mut().set_landed(QString::from(&*landed));
        self.as_mut().set_reveal_last(QString::from(&*reveal_last));
        self.as_mut().set_pre_notice(QString::from(&*pre_notice));
        self.as_mut()
            .set_history_last(QString::from(&*history_last));
        self.as_mut()
            .set_send_to_last(QString::from(&*send_to_last));
        self.as_mut().set_reveal_clicks(clicks);
        self.as_mut().set_reveal_acted(acted);
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

    pub fn drag(mut self: Pin<&mut Self>, x: f64, y: f64, unlocked: bool) {
        let index = *self.index();
        mask::with(|m| m.drag(index.max(0) as usize, x, y, unlocked));
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

    /// §5.7.11 step 4's four. `set_font_size` takes the same `max(0)` that
    /// `apply_zoom_percent` needs, for the same reason: the range is [`crate::annotate`]'s
    /// answer and a negative number must not become 96 by way of `as u32`. The reload is
    /// what makes the knob bite *during* a gesture - the open field's size and box are
    /// both read out of the style the call just moved.
    pub fn set_font_size(mut self: Pin<&mut Self>, size: i32) {
        mask::with(|m| m.ink.set_font_size(size.max(0) as u32));
        self.as_mut().reload();
    }

    pub fn set_align(mut self: Pin<&mut Self>, code: i32) {
        mask::with(|m| m.ink.set_align(code));
        self.as_mut().reload();
    }

    pub fn apply_text_bg(mut self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.ink.set_text_bg(on));
        self.as_mut().reload();
    }

    pub fn apply_text_outline(mut self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.ink.set_text_outline(on));
        self.as_mut().reload();
    }

    /// §5.7.14's 擦除到透明. The reload is the whole of it on this side: the mode moves
    /// the pen, and the row the user just clicked reads both the switch and whose tool it
    /// is off the readback rather than off the click.
    pub fn apply_erase_transparent(mut self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.ink.set_erase_transparent(on));
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

    /// §5.5.2's 复制. The `reload` is not for the caller's benefit - the cell that was
    /// clicked has nothing to redraw - it is for `message`, which every window reads
    /// out of the one mask state, and for the sentence to be on screen at all the
    /// toolbar has to re-read it after the write rather than before.
    pub fn copy_selection(mut self: Pin<&mut Self>) -> bool {
        let done = mask::with(|m| m.copy_out());
        self.as_mut().reload();
        done
    }

    /// §5.5.2's 另存为. `to_string` rather than a borrowed `&str` because the path is
    /// Qt's to keep: `MaskState::save_out` hands it to `std::path`, which wants an
    /// owned name on a Windows drive that may not be one this process can read.
    pub fn save_selection(mut self: Pin<&mut Self>, path: &QString, filter: &QString) -> bool {
        let target = path.to_string();
        let chosen = filter.to_string();
        let done = mask::with(|m| m.save_out(&target, &chosen));
        self.as_mut().reload();
        done
    }

    pub fn tool_names(&self) -> QString {
        QString::from(&*annotate::tool_names())
    }

    pub fn tool_hints(&self) -> QString {
        QString::from(&*annotate::tool_hints())
    }

    pub fn ratio_names(&self) -> QString {
        QString::from(&*mask::ratio_names())
    }

    pub fn ratio_hints(&self) -> QString {
        QString::from(&*mask::ratio_hints())
    }

    /// §5.3.8 的芯片：改动落在 [`mask::MaskState`] 那一格，所以递完就要 `reload` —— 与
    /// [`Self::select_tool`] 同一条理由（"芯片亮哪格"是 QML 绑的读数，不是它自己记的账）。
    pub fn pick_ratio(mut self: Pin<&mut Self>, index: i32) -> bool {
        let took = mask::with(|m| m.pick_ratio(index));
        self.as_mut().reload();
        took
    }

    /// 同上，而 `spec` 是文本框里那句话的所有权移交（`&QString` → `to_string`，与
    /// [`Self::save_selection`] 同一条理由）。那句话**总是**被记下，返回值只说锁上没上
    /// （[`mask::MaskState::set_ratio_spec`]），所以 `false` 之后仍要 `reload`：文本框点灰
    /// 靠的就是这一次读回来的 [`Self::ratio_live`]。
    pub fn apply_ratio_spec(mut self: Pin<&mut Self>, spec: &QString) -> bool {
        let typed = spec.to_string();
        let took = mask::with(|m| m.set_ratio_spec(&typed));
        self.as_mut().reload();
        took
    }

    /// §5.3.6：尺寸那两格文本框的提交。改动落在 [`mask::MaskState`] 的 `hole` 与（撞锁时）
    /// 芯片那一格，所以递完就要 `reload` —— 与 [`Self::pick_ratio`] 同一条理由，而这里要的
    /// 读数比它多：那行 W×H、锚点那一角、芯片亮哪格，三处都是这一次改动的后果。
    pub fn apply_size(mut self: Pin<&mut Self>, w: i32, h: i32) -> bool {
        let took = mask::with(|m| m.type_size(w, h));
        self.as_mut().reload();
        took
    }

    pub fn size_anchor_names(&self) -> QString {
        QString::from(&*mask::size_anchor_names())
    }

    /// §5.3.7 第一、二步的单位字样表 —— 与 [`Self::size_anchor_names`] 同一条理由：页只做一次
    /// 下标，字样与序号的主人在 Rust。
    pub fn unit_names(&self) -> QString {
        QString::from(&*mask::unit_names())
    }

    /// §5.3.7 芯片的悬停句 —— 与 [`Self::unit_names`] 同一条理由，两句都在 Rust
    /// （[`crate::mask::UNIT_HINTS`]），页只做一次下标。
    pub fn unit_hints(&self) -> QString {
        QString::from(&*mask::unit_hints())
    }

    /// §5.3.7 第二步：改动落在 [`mask::MaskState`] 的 `unit`，而它动的不是一格读数而是
    /// **那对读数怎么算出来的** —— 屏幕上有三处跟着它走（§5.3.5 那行 W×H、sizeBar 那行
    /// 「当前 …」、以及哪枚芯片亮着），所以递完就要 `reload`，与 [`Self::apply_size`] 同理。
    pub fn set_unit(mut self: Pin<&mut Self>, code: i32) -> i32 {
        let landed = mask::with(|m| m.set_unit(code));
        self.as_mut().reload();
        mask::unit_code(landed)
    }

    /// P91：`sendToMenuNames()` 的 Rust 侧 —— 从 [`settings::output_cfg`] 走
    /// [`settings::pipeline_send_to`] 拿校验过的 `(name, exe)` 段，
    /// [`settings::send_to_menu_names_joined`] 联成 `|` 分隔串交给 QML。
    /// 空集合 ⇒ 空串；而 `"".split("|")` 在 JS 里是**一段空串**而不是零段，所以 P93
    /// 的 Repeater 接线必须先判空再喂模型（这一格由 P92 的单测钉住）。
    pub fn send_to_menu_names(&self) -> QString {
        let o = settings::output_cfg();
        let (final_ok, _, _) = settings::pipeline_send_to(&o.send_to_apps);
        QString::from(&*settings::send_to_menu_names_joined(&final_ok))
    }

    /// P92：`shareMenuNames()` 的 Rust 侧 —— 从 [`settings::output_cfg`] 走
    /// [`settings::pipeline_share_targets`] 拿白名单内的 known 段，
    /// [`settings::share_menu_names_joined`] 联成 `|` 分隔串交给 QML。与 P91 那一枚
    /// 对称；空集合 ⇒ 空串。
    pub fn share_menu_names(&self) -> QString {
        let o = settings::output_cfg();
        let (known, _, _) = settings::pipeline_share_targets(&o.share_targets);
        QString::from(&*settings::share_menu_names_joined(&known))
    }

    /// P181：`bandColors()` 的 Rust 侧 —— 从 [`settings::annotation_cfg`] 取档里那一排，
    /// [`settings::band_colors_joined`] 归一并 `|` 联起来。与 [`Self::send_to_menu_names`]
    /// 同一分工：名单只有档一个主人，页上那一次 `.split("|")` 是它唯一的读者路径。
    ///
    /// 每一扇遮罩窗创建的那一刻读一次（`AnnotationToolbar.qml` 里那两枚 `readonly property`
    /// 随面板求值一次），与 [`crate::settings::card_cfg`] 同一条理由：改完档再截一张，
    /// 屏幕上就该是新档里那一排，而不是进程启动那一刻的快照。
    pub fn band_colors(&self) -> QString {
        QString::from(&*settings::band_colors_joined(
            &settings::annotation_cfg().palette,
        ))
    }

    /// §5.5.10 步骤 2 的按下：`index` 是 Repeater 的那一枚，越界与否由 Rust 答（同一份清单
    /// 只在 [`crate::mask::MaskState::send_to_target`] 里数一次）。`reload` 与 复制/保存
    /// 那一趟是同一条理由：话活在唯一的 mask 状态里，两扇窗都得在**动手之后**重读一次才看得见。
    pub fn send_to_target(mut self: Pin<&mut Self>, index: i32) -> bool {
        let done = mask::with(|m| m.send_to_target(index.max(0) as usize));
        self.as_mut().reload();
        done
    }

    /// §10 第 43 项 ①'s 存完再钉图. Same path handling as [`Self::save_selection`],
    /// same reason: the name is Qt's to keep and `std::path` wants it owned.
    pub fn save_and_pin(mut self: Pin<&mut Self>, path: &QString, filter: &QString) -> bool {
        let target = path.to_string();
        let chosen = filter.to_string();
        let done = mask::with(|m| m.save_then_pin(&target, &chosen));
        self.as_mut().reload();
        done
    }

    /// §5.5.4's 快速保存. Nothing to convert, because nothing comes from a dialog: the
    /// name and the directory are Rust's `[output]` reading, taken when the mask opened.
    /// The `reload` is the same one 复制 needs - the sentence lives in the one mask state
    /// and both windows have to re-read it after the write rather than before.
    pub fn quick_save(mut self: Pin<&mut Self>) -> bool {
        let done = mask::with(|m| m.quick_out());
        self.as_mut().reload();
        done
    }

    /// §5.5.4 step 4 的点击。走的是和落盘自动揭示同一条 [`crate::mask::MaskState`] 里的
    /// 路，所以两条路给门的是一句话而不是两句。
    pub fn reveal_landed(mut self: Pin<&mut Self>) -> bool {
        let done = mask::with(|m| m.reveal_landed());
        self.as_mut().reload();
        done
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
