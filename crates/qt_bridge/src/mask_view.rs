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
        /// §5.3.2 第三步「系统高亮窗口边界」那一块框，**这一扇窗自己的**窗内 DIP，与
        /// [`Self::hole_x`] 那一族同一把尺子（[`crate::mask::MaskState::local_dip`] 一位
        /// 作者）。`hover_live` 才是「该不该画」的判据 —— 别看尺寸：0×0 画不出东西是巧合，
        /// 不是"这一扇窗上没有可亮的窗"的定义。
        ///
        /// 页上只**绑**这五格、不读返回值，绑的是属性本身而不是 `xxxChanged` 信号 ——
        /// `#[qproperty(bool, hover_live)]` 生成的 notify 名是 `hover_liveChanged` 而不是
        /// 驼峰的 `hoverLiveChanged`，绑错名字是**静默**的（本仓踩过两次，见
        /// [`Self::pen_alpha`] 头上那段）。绑值这条路没有这个名字可绑错。
        #[qproperty(bool, hover_live)]
        #[qproperty(i32, hover_x)]
        #[qproperty(i32, hover_y)]
        #[qproperty(i32, hover_w)]
        #[qproperty(i32, hover_h)]
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
        /// The same pen's fourth byte, as the percent §5.7.2 step 2 / §5.7.21 step 3 read
        /// out. A property of its own rather than something QML decodes off `pen_rgb`,
        /// because `pen_rgb` is the *chip's* colour and the chip's list carries no alpha:
        /// the wheel moves a byte the band cannot show, so a readout taken from what the
        /// band lights would be a reading of the wrong thing.
        #[qproperty(i32, pen_alpha)]
        #[qproperty(bool, dashed)]
        #[qproperty(bool, filled)]
        /// §5.7.9's 块大小 / §5.7.10's 模糊强度: whether the row belongs on screen at all,
        /// what the one number is called while this tool is in hand, and what it is. The
        /// fourth row of the shape `zoom_tool` gives 放大, and Rust answers all three for
        /// the same reason - the index of 马赛克 in [`crate::annotate::TOOLS`] is Rust's
        /// number, and §5.7.1's per-tool memory can hand either tool back a strength no
        /// button has ever clicked.
        #[qproperty(bool, effect_tool)]
        #[qproperty(i32, effect_intensity)]
        #[qproperty(QString, effect_caption)]
        /// §5.7.10 step 2's 形状, and the flag that says whether the tool in hand drags a
        /// brush at all - all three pens draw, so this is not a light about nothing. 三档是
        /// 一颗序号（`annotate` 里 `BRUSH_SHAPES` 那一行数的），读回来的同 `arrow_head`。
        #[qproperty(bool, brush_tool)]
        #[qproperty(i32, brush_shape)]
        /// §5.7.6 step 2「选择单向、双向或**其他箭头样式**」里的那其他样式：三档一颗序号，
        /// 加上「手里这支笔是不是箭头」那扇门（单向与双向都算 —— 它们本是 `TOOLS` 上两枚工具，
        /// 样式才是这一格）。门与序号都是读回来的，同 `brush_tool` 的理由。
        #[qproperty(bool, arrow_tool)]
        #[qproperty(i32, arrow_head)]
        /// §5.7.3 step 2「配置圆角半径」的那一个数与它的门。阶梯递方向、`annotate` 夹两端，
        /// 于是屏上读的半径与落在文档里的是同一个半径。
        #[qproperty(bool, rounded_tool)]
        #[qproperty(i32, corner_radius)]
        /// §5.7.12 step 2「设置起始编号」与它的门：读回来的是 `Document` 计数器此刻的那一格，
        /// 不是界面自己记的数，所以撤销一枚编号之后屏上那一格仍然对得上。
        #[qproperty(bool, number_tool)]
        #[qproperty(i32, counter_start)]
        /// §5.7.12 step 5「删除中间编号时可选择是否自动重排」的那一枚选择键。亮着的下一次删除
        /// 记的是**一条**装着「删除 + 重排」的编辑（[`crate::annotate::Layer::delete_selected`]）；
        /// 读回来而不是界面自己记，同 `counter_start` 的理由 —— 这一格只有一个作者，就是层上那一个。
        #[qproperty(bool, auto_renumber)]
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
        /// 框里预先放着的那一串与它所带的字体名 —— 两格都是**这一只框**的读数，不是画笔的：
        /// §5.7.11 落下的新框是空串和画笔的族名，§5.7.16 步 3 改字那一只是对象自己的话和
        /// 对象自己的字体。`pen_family` 仍是工具栏那一排亮着的字体名（画笔），所以一次改字
        /// 不会把工具栏的点改到对象那一族上去（#281：同一件事只有一位作者）。
        /// 两格都必须写在 `text_editing` 之前 —— 页上在可见性翻过去那一刻 imperative 地读它们。
        #[qproperty(QString, text_seed)]
        #[qproperty(QString, text_family)]
        /// 框里那一串字的颜色（`0xRRGGBB`，与上面 `pen_rgb` 同一打包），同一只框的第三格读数：
        /// §5.7.16 步 3 改字那一只是对象自己的 `Style.color`，§5.7.11 落新框那一只是画笔的。
        /// 页上那一行 `color:` 绑的就是这一格 —— 绑 `pen_rgb` 的字段会在框关掉那一刻换一次色，
        /// 因为提交上去的那一枚保留它自己的颜色。它是**声明式绑定**（不像上面两格在可见性那一
        /// 步被 imperative 地读），所以写在 `text_editing` 前后都一样，这里仍归进同一组。
        #[qproperty(i32, text_rgb)]
        /// §5.7.11 step 4's knobs, and the flag that says whether the tool in hand
        /// is the one they belong to - the same shape `zoom_tool` gives 放大's row.
        /// These are the four of the step's five that sit on a ladder or a switch; the
        /// fifth, 字体, is a name rather than a rung and has its own `QString` below.
        /// `pen_font` is the *pen's* size in device pixels and `text_font` above is the
        /// open field's in DIP: the toolbar has to light the cell whose number the click
        /// sends, and the field has to be sized in the units its window draws in.
        #[qproperty(bool, text_tool)]
        #[qproperty(i32, pen_font)]
        #[qproperty(i32, text_align)]
        #[qproperty(bool, text_bg)]
        #[qproperty(bool, text_outline)]
        /// §5.7.11 那句规则的「颜色和宽度」两半，读回的是**渲染那一层要用的那两个数**
        /// （[`crate::annotate::Layer::outline_colour`] / [`crate::annotate::Layer::outline_width`]
        /// 走的是 `Style::outline_for`）。面板不自己判「没人配过时该让位给谁」：那样同一圈边
        /// 就有了两个作者，而屏幕上那一圈和工具栏里亮着的那枚会各答一枚（#281）。
        #[qproperty(i32, outline_rgb)]
        #[qproperty(i32, outline_width)]
        /// §5.7.11 step 4's 字体, read back as well as set. It arrives through `reload`
        /// like the four above: the config file's row can move it, [`Self::set_family`]
        /// can move it, and a per-tool memory can hand it back on a tool the buttons have
        /// never typed into - so the page shows the answer the pen actually carries, and
        /// not the string somebody last typed.
        #[qproperty(QString, pen_family)]
        /// §5.7.14's 擦除到透明, and the flag that says whether the tool in hand is the
        /// eraser it belongs to - the third row of the shape `zoom_tool` and `text_tool`
        /// give the two before it. The flag is Rust's because the mode is a pen field
        /// like any other, and §5.7.1's per-tool memory can hand the eraser back a mode
        /// the buttons have never clicked.
        #[qproperty(bool, eraser_tool)]
        #[qproperty(bool, erase_transparent)]
        /// §5.7.16/§5.7.17 那一行的三枚读数，全部**读回**而不是界面自己记：
        ///
        /// * `picked_count` —— 这一串里有几枚。0 是"没选中东西"，那一行因此整行不占高度；
        ///   界面上没有第二处数过这串（`objects` 是这一层有几枚对象，与选中无关）。
        /// * `picked_rotation` —— 面板所报那一枚的角度，也就是 §5.7.15 step 3「实时显示旋转
        ///   结果」在屏幕上的那一个数。报的是**最上面**那一枚的，与 [`Selection::topmost`] 给
        ///   面板报样式同一口径。
        /// * `marquee_mode` —— 框选那一档开着没有（§5.7.17 step 1）。
        ///
        /// [`Selection::topmost`]: falcon_core::annotation::Selection::topmost
        #[qproperty(i32, picked_count)]
        #[qproperty(i32, picked_rotation)]
        #[qproperty(bool, marquee_mode)]
        /// §5.7.3 step 4 的那一枚圆角把手在这一扇窗上的位置（window-local DIP），外加「此刻
        /// 有没有一枚可画」。页上只把它**画**出来，与上面那八枚同规（drawn, never clicked）：
        /// 一次按下归谁，是 `pressAt` 里 core 那一位作者（`grip_at_any`）判的，页上再摆一枚
        /// MouseArea 就是让光标与拖拽起点各答一次「用户握住了哪一枚」。位置本身也在 core
        /// （`grip_point`）与 [`crate::mask::MaskState::view_data`] 那一次除法，界上一处也不算。
        #[qproperty(bool, radius_live)]
        #[qproperty(i32, radius_x)]
        #[qproperty(i32, radius_y)]
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
        /// §5.7.21 第二步的那两句：改完了（或被拒了）的那一句，与「这一排又被改了一次」那枚钟。
        ///
        /// `band_revision` 存在的唯一理由写在 [`crate::mask::MaskState::band_revisions`]：
        /// `bandColors()` 是一枚函数，QML 的绑定不会为一次函数调用重发，所以页上那一排要跟着档走，
        /// 就得有一个真的会变的数被绑进去。名字用下划线、不按驼峰（PRD §3.6 约束 8 那一族）。
        #[qproperty(QString, band_last)]
        #[qproperty(i32, band_revision)]
        /// §5.4.1 —— 放大镜这一块**当前**的读数，由 [`Self::updateMagnifier`] 逐次 hover 里
        /// 一次装配好、一次锁里交回。字段这么薄是因为像素不进这一层：像素躺在 Qt 的纹理
        /// 仓库里，这里只带**键**（同 [`Self::key`] / [`Self::overlay_key`] 那一族的理由）。
        ///
        /// **为什么不走 [`Self::reload`]**：reload 一次要问十几次 `MaskState` 的字段、拼一次
        /// 大 tuple；放大镜跟着指针走一万下，那一族负担就压到 hover 每一帧上了。这一族
        /// 只由 [`Self::updateMagnifier`] 一手填，读的是**同一次**锁里的同一份 [`crate::mask::MagnifierData`]，
        /// 因此不会出现"坐标是这一趟的、像素是上一趟的"（同 [`Self::ratio_spec`] 那一族
        /// 「一次锁一次样本」的口径）。
        ///
        /// 名字全用**下划线**：cxx-qt 生成的 notify 是 `<field>Changed` 而不是驼峰 `<field>Changed`
        /// （驼峰名会**静默绑不上**，PRD §3.6 约束 8 一族），QML 侧只按值绑、不按名绑 signal。
        #[qproperty(bool, magnifier_show)]
        #[qproperty(QString, magnifier_key)]
        #[qproperty(i32, magnifier_w)]
        #[qproperty(i32, magnifier_h)]
        #[qproperty(QString, magnifier_coord)]
        #[qproperty(QString, magnifier_color)]
        #[qproperty(i32, magnifier_rgb)]
        #[qproperty(bool, magnifier_has_color)]
        #[qproperty(bool, magnifier_grid)]
        #[qproperty(i32, magnifier_pixel_w)]
        #[qproperty(i32, magnifier_zoom)]
        /// §5.3.9 第 4 步 —— 这一块面板**摆在哪一格**（窗内 DIP）：这一趟真正去采样的那一点
        /// 在本窗的读法，与上面那一族同一次装配（[`crate::mask::MagnifierData::anchor_x`]）。
        ///
        /// 为什么位置也进了桥：以前它只有页上 `win.cursorX/cursorY` 那两格，写它的三处全是
        /// 鼠标事件。键盘把选区挪过一格之后屏幕上**没有**新的鼠标事件，那两格就留在原地 ——
        /// 于是"面板摆在指针那儿、里面画的是另一条边的像素"。指针那条腿今天仍然只叫
        /// [`Self::updateMagnifier`]，页上不再自己留一份"此刻在哪"的副本（#281）。
        #[qproperty(f64, magnifier_anchor_x)]
        #[qproperty(f64, magnifier_anchor_y)]
        /// §5.19.3「放大镜外观」那四枚 `[appearance] magnifier_*` 键在这一族的**同一趟**
        /// hover 里的读数 —— 名字与档里的键一一对上（`magnifier_background` /
        /// `magnifier_foreground` / `magnifier_border` / `magnifier_corner_radius`），因为
        /// 页上、桥里、档上三处指认同一枚键时，第二个名字就是第二个作者（#281）。
        ///
        /// 两枚颜色是 [`crate::mask::MagnifierLook`] 归一过的那一份（`#RRGGBB`，alpha 不是
        /// 255 时 `#AARRGGBB`），页上直接 `color:` 绑、不再自己解析；坏串在桥里就落回出厂
        /// 那一枚，所以这一格**永远不会**是读不出来的颜色 —— 除了第一帧还没 hover 过的空串，
        /// 那一臂归页上 `!== ""` 兜（与 `CustomCaptureWindow.qml` 的 `accent_fill` 同一族）。
        #[qproperty(QString, magnifier_background)]
        #[qproperty(QString, magnifier_foreground)]
        #[qproperty(i32, magnifier_border)]
        #[qproperty(i32, magnifier_corner_radius)]
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
        /// §5.7.4 step 3「按住约束键可创建正圆」的那只手：界面把**当下这一枚鼠标事件**里那
        /// 枚修饰键按着没有递进来，正在拖的那一笔框于是锁成 1:1。每一寸移动都递一次，
        /// 起笔时 Rust 自己归 false（`annotate::Layer::press`），所以这一格不会有一把
        /// "键早松了、笔还锁着"的门；它也不走 `dragTo` 的 `unlocked` —— 那一枚说的是
        /// **松开**选区的比例锁（§5.3.8），同一只手不能一个键说两种相反的话。
        /// 这一格不 publishes 任何东西，所以后面不跟 `reload`。
        #[qinvokable]
        #[cxx_name = "holdConstraint"]
        fn hold_constraint(self: Pin<&mut Self>, on: bool);
        /// §5.7.18 step 2「按住 Ctrl 拖动」的那只手：页在**每一枚按下之前**把当下 `mouse.modifiers`
        /// 里那枚 Ctrl 递进来，箭头工具在选区里那一拖拖的就是副本。它与 `holdConstraint` 分开两格
        /// 而不是共用一枚，因为两枚键说的话不同（约束键说的是拖出来的框锁 1:1、每一寸移动都重写；
        /// 这一枚说的是这一趟拖的是谁、只在按下那一刻有效，`annotate::Layer::press` 读一次即归
        /// false）。中途松手不把已经开始的克隆改回移动：一只手不能一半拖副本、一半拖原物。
        #[qinvokable]
        #[cxx_name = "holdClone"]
        fn hold_clone(self: Pin<&mut Self>, on: bool);
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

        /// §5.7.16 步 3 的「文字内容」：F2 把选中的那一枚 文本 就地摊开成页上那一枚
        /// `TextEdit`（同一只框、同一个 `finishInk` 收尾）。`true` = 框开了，调用者随之
        /// 把这一趟递到别的窗上去；没开时屏幕上那句「为什么没反应」是
        /// [`crate::mask::MaskState::edit_text`] 的，不是这一层的话。
        #[qinvokable]
        #[cxx_name = "editText"]
        fn edit_text(self: Pin<&mut Self>) -> bool;

        /// Esc: `true` when it cleared a selection, `false` when the caller should
        /// cancel the capture. One keystroke must not both clear and cancel.
        #[qinvokable]
        #[cxx_name = "stepBack"]
        fn step_back(self: Pin<&mut Self>) -> bool;

        /// Arrow keys (§5.3.9 第一到三步): `edge` resizes the bottom-right instead of moving.
        ///
        /// 页上叫这一手就够了 —— §5.3.9 第 4 步「放大镜同步显示边缘像素」也在这条腿里一次做完
        /// （见 [`crate::mask_view::MaskViewRust::nudge`]），所以页上不需要、也不许再自己算一遍
        /// "刚被挪掉的那条边在哪"：那是 core 的一件一次算好的事（`falcon_core::geometry::nudge_sample`）。
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

        /// §5.7.2 step 2's pen colour, as three 0..255 channels. The pen's alpha is not
        /// one of them and is not reset by them either: it is the fourth byte of the same
        /// colour, and the chip the click comes from has no alpha to offer - see
        /// [`crate::annotate::Layer::alpha_percent`] for the knob that does.
        #[qinvokable]
        #[cxx_name = "setColor"]
        fn set_color(self: Pin<&mut Self>, r: i32, g: i32, b: i32);

        /// §5.7.21 step 3's 「滚轮调整透明度」: one rung of the ladder per notch, in the
        /// direction the wheel turned, from wherever the pen stands. A step rather than a
        /// percent setter because the rung size, the floor and the ceiling are
        /// [`crate::annotate`]'s numbers, and a QML that worked out the next percent would
        /// be a second ladder able to disagree with the one Rust clamps to.
        #[qinvokable]
        #[cxx_name = "stepAlpha"]
        fn step_alpha(self: Pin<&mut Self>, notches: i32);

        /// §5.7.9 / §5.7.10 step 2's 强度 cells and wheel. `apply_` because
        /// `effect_intensity` is a qproperty now, so `set_effect_intensity` is already the
        /// name of its setter.
        #[qinvokable]
        #[cxx_name = "applyEffectIntensity"]
        fn apply_effect_intensity(self: Pin<&mut Self>, value: i32);

        /// §5.7.10 step 2's 形状 (圆头 / 方头 / 扁头，序号按 `annotate` 里 `BRUSH_SHAPES`
        /// 那张表数). `apply_` for the same duplicate-name
        /// reason `apply_effect_intensity` needs it: `brush_shape` is a qproperty too.
        #[qinvokable]
        #[cxx_name = "applyBrushShape"]
        fn apply_brush_shape(self: Pin<&mut Self>, code: i32);

        /// §5.7.6 step 2 的「其他箭头样式」：三档之一，序号按 `annotate` 里 `ARROW_HEADS`
        /// 那一张表数。`apply_` 是因为 `arrow_head` 已经是 qproperty（同
        /// [`Self::apply_dashed`] 那一族的理由）。
        #[qinvokable]
        #[cxx_name = "applyArrowHead"]
        fn apply_arrow_head(self: Pin<&mut Self>, code: i32);

        /// §5.7.3 step 2「配置圆角半径」的那把阶梯：QML 只递方向，格宽有多宽、地板与天花板各
        /// 是多少都在 `annotate`（与 [`Self::step_alpha`] 同一分工 —— 一页里再写一把梯就会有一
        /// 把与这把不一致）。
        #[qinvokable]
        #[cxx_name = "stepCornerRadius"]
        fn step_corner_radius(self: Pin<&mut Self>, notches: i32);

        /// §5.7.12 step 2 的起始编号阶梯：与 [`Self::step_corner_radius`] 同一分工，QML 只递
        /// 方向。`step_` 不撞 qproperty 的名字，所以这一族不需要 `apply_` 前缀。
        #[qinvokable]
        #[cxx_name = "stepCounterStart"]
        fn step_counter_start(self: Pin<&mut Self>, notches: i32);

        /// §5.7.12 step 5 的那一枚「是否自动重排」。`apply_` 前缀的理由与
        /// [`Self::apply_marquee`] 同一句：`auto_renumber` 已经是 qproperty，
        /// `set_auto_renumber` 那个名字被它的 setter 占了。
        #[qinvokable]
        #[cxx_name = "applyAutoRenumber"]
        fn apply_auto_renumber(self: Pin<&mut Self>, on: bool);

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
        /// `text_tool` is lit, plus 字体 below them: step 4 names five knobs, and the fifth
        /// is a name the user types rather than a cell to light. `apply_` on the last two for
        /// the duplicate-name reason `apply_dashed` gives: `text_bg` and `text_outline` are
        /// qproperties now.
        #[qinvokable]
        #[cxx_name = "setFontSize"]
        fn set_font_size(self: Pin<&mut Self>, size: i32);
        #[qinvokable]
        #[cxx_name = "setAlign"]
        fn set_align(self: Pin<&mut Self>, code: i32);
        /// §5.7.11 step 4's 字体，那一格栅窗里唯一的**一串字**：递 QString 而不是第几枚芯片，
        /// 因为族名没有梯可站（[`crate::annotate::Layer::set_font_family`]）。
        #[qinvokable]
        #[cxx_name = "setFamily"]
        fn set_family(self: Pin<&mut Self>, family: &QString);
        #[qinvokable]
        #[cxx_name = "applyTextBg"]
        fn apply_text_bg(self: Pin<&mut Self>, on: bool);
        #[qinvokable]
        #[cxx_name = "applyTextOutline"]
        fn apply_text_outline(self: Pin<&mut Self>, on: bool);

        /// §5.7.11 那句规则的「颜色和宽度」两半 —— 开关之外还缺的那两只手。宽度那一枚
        /// `apply_` 是因为 `outline_width` 已经是 qproperty，`set_outline_width` 那个名字被
        /// 它的 setter 占了（同 [`Self::apply_text_outline`] 那一族）；颜色那一枚收三字节，
        /// 与 [`crate::annotate::Layer::set_outline_colour`] 一样不碰那一圈边自己的 alpha。
        #[qinvokable]
        #[cxx_name = "setOutlineColour"]
        fn set_outline_colour(self: Pin<&mut Self>, r: i32, g: i32, b: i32);
        #[qinvokable]
        #[cxx_name = "applyOutlineWidth"]
        fn apply_outline_width(self: Pin<&mut Self>, px: i32);

        /// §5.7.14's 擦除到透明. `apply_` on this one for the duplicate-name reason
        /// `apply_text_bg` gives: `erase_transparent` is a qproperty now, so
        /// `set_erase_transparent` is already its setter's name.
        #[qinvokable]
        #[cxx_name = "applyEraseTransparent"]
        fn apply_erase_transparent(self: Pin<&mut Self>, on: bool);

        /// §5.7.17 step 4 的「移动/复制/删除/样式调整」里那两枚**点得动**的钮，加上
        /// §5.7.15 step 2 的角度：三枚都答「这回真改了东西没有」，与 [`Self::undo_step`] 同一
        /// 口径 —— 一串空选区、一组全锁着、或整组都是不转的直线，都该是 `false` 而不是"按了
        /// 没反应"。样式那一半不需要 invokable：笔上那十几枚 apply_* 已经顺带改这串
        /// （见 [`crate::annotate::Layer`] 的选中重编辑那一批 setter）。
        #[qinvokable]
        #[cxx_name = "deletePicked"]
        fn delete_picked(self: Pin<&mut Self>) -> bool;
        #[qinvokable]
        #[cxx_name = "clonePicked"]
        fn clone_picked(self: Pin<&mut Self>) -> bool;
        #[qinvokable]
        #[cxx_name = "stepRotation"]
        fn step_rotation(self: Pin<&mut Self>, notches: i32) -> bool;

        /// §5.7.17 step 1 的「进入选择模式」那一档。`apply_` 是因为 `marquee_mode` 已经是
        /// 一枚 qproperty，`set_marquee_mode` 那个名字被它的 setter 占了（同
        /// [`Self::apply_dashed`] 那一族）。
        #[qinvokable]
        #[cxx_name = "applyMarquee"]
        fn apply_marquee(self: Pin<&mut Self>, on: bool);

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

        /// §5.8.1 步 2 的那一枚「贴图」：同一份 [`crate::mask::MaskState::delivered`] 直接
        /// 成为浮窗，一份文件也不写。`true` 是「屏幕上多了一张贴图」，而包括"还没有选区"和
        /// "拼不出画面"那两句拒绝在内的话都在 [`crate::mask::MaskState::message`] 里 ——
        /// 与 [`Self::saveAndPin`] 同一个分工：页上不替这一手判第二遍。
        #[qinvokable]
        #[cxx_name = "pinSelection"]
        fn pin_selection(self: Pin<&mut Self>) -> bool;

        /// §5.5.4's 快速保存: the same write 另存为 does, with no dialog and no name from
        /// the user - `[output]` decides where it goes and what it is called. `true` is
        /// "the file is there", and as with the two doors above the sentence is already in
        /// `message` either way, including the one about the directory the machine could
        /// not name and the one about a taken name under `collision = keep_both`.
        #[qinvokable]
        #[cxx_name = "quickSave"]
        fn quick_save(self: Pin<&mut Self>) -> bool;

        /// §5.5.9 的替代分支：本产品没有原生分享面板（WinRT 要包身份），所以「分享」这一枚
        /// 按下去走的是「把这一张写成一份文件，再交给系统默认的看图程序」。`true` 是 shell
        /// 收下了这条请求；话在 [`crate::mask::MaskState::message`] 里，而且是**接**在
        /// [`Self::quick_save`] 那句「已保存到 …」之后的两句话 —— 落盘那一句是这份文件存在的
        /// 证据，门不许把它换掉。三道梯各归哪一处，写在
        /// [`crate::mask::MaskState::share_out`] 头顶。
        #[qinvokable]
        #[cxx_name = "shareOut"]
        fn share_out(self: Pin<&mut Self>) -> bool;

        /// §5.5.4 step 4：通知上那一次点击。`true` 是 shell 收下了这条请求，而话——
        /// 包括"这张图还没有落到任何地方"和"要揭示的东西不在盘上"那两句拒绝——在
        /// [`crate::mask::MaskState::reveal_last`]，和 复制/保存 一样不经返回值传话。
        #[qinvokable]
        #[cxx_name = "revealLanded"]
        fn reveal_landed(self: Pin<&mut Self>) -> bool;

        /// §5.4.1 —— 指针在这一扇窗里落到 `(x, y)`（窗内 DIP）时，放大镜这一块该怎么画。
        /// 页上 `MouseArea.onPositionChanged` 与 `onPressed` 各一次调用它，同一趟里
        /// [`Self::magnifier_*`] 十七格一次改齐（同一次锁、同一份 [`crate::mask::MagnifierData`]
        /// —— 分两次问就可能出现"句子是这一趟的、像素是上一趟的"）。
        ///
        /// **不返回**任何东西：QML 只绑这一族的 qproperty，不读返回值。这样调用点写起来一句
        /// 就够（`view.updateMagnifier(mouse.x, mouse.y)`），页上不必再拿返回值往十七个格上
        /// 分派。
        ///
        /// 键盘那一趟（§5.3.9 第 4 步）走的**不是**这一手：那一趟屏幕上没有新的 `(x, y)` 可递，
        /// 它叫 [`Self::nudgeHole`]，采样点由 core 算（`falcon_core::geometry::nudge_sample`）。
        /// 两条腿共用的只有下面那一处装配（[`Self::apply_magnifier`]）。
        #[qinvokable]
        #[cxx_name = "updateMagnifier"]
        fn update_magnifier(self: Pin<&mut Self>, x: f64, y: f64);

        /// §5.3.2 第二、三步 —— 指针走到 `(x, y)`（窗内 DIP）时，哪一扇窗该被高亮。
        ///
        /// 一次调用改齐 [`Self::hover_live`] 那五格（同一次锁、同一份
        /// [`crate::mask::MaskState`]），与 [`Self::updateMagnifier`] 十七格一次改齐同一条
        /// 口径：分两趟问就会问出「框是这一趟的、缩放是上一趟的」那一族。
        ///
        /// **返回** `true` = 这一趟光标落在了一扇**可截**的窗上。页上不需要读它（五格都在
        /// 那五个属性里），留这一枚是给量具的：`--mask` 那一扇门没有指针注入器（第 46 项 ②），
        /// 而「这一枪到底命中没有」是这一路唯一一句能被外面读到的答案。
        ///
        /// 命中哪一扇、可截图边界是哪一块、不可见与全透的怎么排除 —— 一概不在这里，也不在
        /// [`crate::mask`]：判据只住在 `falcon_core::capture`（[`crate::capture::window_under`]
        /// 头上那段是本路唯一一处指认）。这一手只做搬运和一次窗内换算。
        #[qinvokable]
        #[cxx_name = "hoverWindow"]
        fn hover_window(self: Pin<&mut Self>, x: f64, y: f64) -> bool;

        /// §5.4.2 —— 裸按 `C` 把光标所在那一点的颜色按 [`crate::mask::MaskState::capture`]
        /// 的 `magnifier_color_format` 复制进剪贴板。`true` 是"这一串已经在剪贴板上了"，
        /// `false` 是"没复制出去"，两句话都说在 [`crate::mask::MaskState::message`] 里
        /// （与 复制/保存/发送 同一族"话在 Rust 手里"的口径，页上那行 `message` 自然接得住）。
        ///
        /// **读的是 [`Self::updateMagnifier`] 刚记下的那一点**，不是页上另算一遍的坐标 —— 两件事
        /// 共用同一个"光标此刻在哪"，屏幕上放大镜写着什么色、按 C 复制的就是什么色。
        #[qinvokable]
        #[cxx_name = "pickColor"]
        fn pick_color(self: Pin<&mut Self>) -> bool;

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

        /// §5.7.21 第二步：把这一排里的第 `at` 枚顺着顺序挪 `delta` 格。页上拖出来的那两个格号
        /// 之差就是 `delta`，而**换算与落盘只有一只手** —— 设置页「上一格／下一格」那两只钮在用的
        /// [`crate::settings_view::pal_move`]。这一枚门把手自己不改顺序、不写档：它把同一只手交回的
        /// 那句话放进 [`crate::mask::MaskState::band_last`]，把
        /// [`crate::mask::MaskState::band_revisions`] 加一格，再走一次 `reload` 让页上重读名单。
        ///
        /// 递回来的是那一句**原话**而不是 `bool`：「第 1 枚已经是排头」与「改好了」要在屏幕上说
        /// 两件不同的事，合成一枚 bool 就丢掉拒绝的那半句。
        #[qinvokable]
        #[cxx_name = "moveColor"]
        fn move_color(self: Pin<&mut Self>, at: i32, delta: i32) -> QString;

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
    // §5.3.2 第三步那块高亮框的五格。`Default` 给的 `false` + 全 0 与上面那一族同规：一张
    // 还没 `assign`、还没 hover 过任何一扇窗的窗，页上 `hover_live` 那道门后面什么都不画。
    hover_live: bool,
    hover_x: i32,
    hover_y: i32,
    hover_w: i32,
    hover_h: i32,
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
    // §5.7.21 step 3 的读数。`Default` 给的 0 是「还没读到」，与上面 `pen_rgb` 的 0 同规：
    // 一张还没 `assign` 过的窗不画工具条，也就没人看见这个 0。
    pen_alpha: i32,
    dashed: bool,
    filled: bool,
    // §5.7.9 / §5.7.10 那一行与 §5.7.10 的 形状，同上：0/false/空串就是「无」。
    effect_tool: bool,
    effect_intensity: i32,
    effect_caption: QString,
    brush_tool: bool,
    brush_shape: i32,
    arrow_tool: bool,
    arrow_head: i32,
    rounded_tool: bool,
    corner_radius: i32,
    number_tool: bool,
    counter_start: i32,
    auto_renumber: bool,
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
    // 框里预先放着的那一串：§5.7.11 新的一枚是空的，§5.7.16 步 3 改字那一只是对象自己那句话。
    // 页上在**可见性翻过去那一刻**读一次（`onVisibleChanged`），所以这一格必须在
    // `text_editing` 之前写 —— 先亮字段再写字，字段读到的是上一趟的空串，而空串交回
    // `commit_typing` 的答案是「不建条」，一次改字就这么静悄悄没了。
    text_seed: QString,
    // 框里那一串字该用哪一族画。`Default` 给的空串读作「没有框开着」，页上那一只字段绑它的
    // 时候也在 `text_editing` 门后，所以空串不会把占位句挤掉。
    text_family: QString,
    // 框里那一串字的颜色，`0xRRGGBB`：与上面两格同一位作者（`Layer::typing_colour` 走
    // `box_style`），改字那一只读对象自己的那一枚。`Default` 的 0 与 `pen_rgb` 同规 = 还没读到。
    text_rgb: i32,
    text_tool: bool,
    pen_font: i32,
    text_align: i32,
    text_bg: bool,
    text_outline: bool,
    // 那一圈边的颜色与宽度。`Default` 给的 0/0 与上面那一族同规：还没 `assign` 过的窗不画
    // 工具条，而这一排又只在 `text_outline` 亮着时才占位。
    outline_rgb: i32,
    outline_width: i32,
    pen_family: QString,
    eraser_tool: bool,
    erase_transparent: bool,
    // §5.7.16/§5.7.17 那一行的三格。`Default` 给的 0/0/false 与上面那一族同规：还没 `assign`
    // 过的窗报的是「没选中东西」，页上那一整行都在 `picked_count > 0` 的门后面。
    picked_count: i32,
    picked_rotation: i32,
    // §5.7.3 步 4 那一枚圆角把手在本窗的位置。`Default` 给的 false/0/0 读作「此刻没有一枚可画」，
    // 与上面那一族同规：还没 `assign` 过的窗不画工具条，也就没有把手。
    radius_live: bool,
    radius_x: i32,
    radius_y: i32,
    marquee_mode: bool,
    message: QString,
    landed: QString,
    reveal_last: QString,
    pre_notice: QString,
    send_to_last: QString,
    history_last: QString,
    band_last: QString,
    band_revision: i32,
    reveal_clicks: i32,
    reveal_acted: i32,
    // §5.4.1 —— 放大镜那一族。`Default` 给的 `false` + 空串 + 零尺寸就是"这一场还没 hover
    // 过、页上不画任何东西"，与上面 `size_w`/`ratio_spec` 那一族同规：一张还没 `assign`
    // 过的窗，页上 `visible: view.magnifier_show && surface.containsMouse` 就是 `false`。
    // 下面那四枚 §5.19.3 的外观同规（空串 = 还没第一趟 hover），页上 `!== ""` 那一臂兜住，
    // 一旦第一趟 `updateMagnifier` 落地就再也不会空。
    magnifier_show: bool,
    magnifier_key: QString,
    magnifier_w: i32,
    magnifier_h: i32,
    magnifier_coord: QString,
    magnifier_color: QString,
    magnifier_rgb: i32,
    magnifier_has_color: bool,
    magnifier_grid: bool,
    magnifier_pixel_w: i32,
    magnifier_zoom: i32,
    // §5.3.9 第 4 步 —— 面板摆在哪一格（窗内 DIP）。`Default` 的 `0.0` 只可能出现在还没
    // 任何一趟 `updateMagnifier`/`nudgeHole` 落地之前，而那一趟之前 `magnifier_show` 也是
    // `false`：一块不画的面板摆在左上角，屏幕上谁也看不见它。
    magnifier_anchor_x: f64,
    magnifier_anchor_y: f64,
    // §5.19.3 那四枚 `[appearance] magnifier_*` 键的读数，与上面那一族同一次
    // `updateMagnifier` 落地（所以「面板换了长相」与「面板画不画」永远不会来自两趟）。
    magnifier_background: QString,
    magnifier_foreground: QString,
    magnifier_border: i32,
    magnifier_corner_radius: i32,
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
            band_last,
            band_revision,
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
                    m.ink.alpha_percent(),
                    m.ink.dashed(),
                    m.ink.filled(),
                    m.ink.effect_tool(),
                    m.ink.effect_intensity(),
                    m.ink.effect_caption(),
                    m.ink.brush_tool(),
                    m.ink.brush_shape_code(),
                    m.ink.arrow_tool(),
                    m.ink.arrow_head_code(),
                    m.ink.rounded_tool(),
                    m.ink.corner_radius(),
                    m.ink.number_tool(),
                    m.ink.counter_start(),
                    m.ink.auto_renumber(),
                    m.ink.zoom_tool(),
                    m.ink.zoom_percent(),
                    m.ink.zoom_border(),
                    m.ink.connection_line(),
                    m.ink.text_tool(),
                    m.ink.font_px() as i32,
                    m.ink.align_code(),
                    m.ink.text_bg_on(),
                    m.ink.text_outline_on(),
                    // 那一圈边的两格与开关同一次锁：分两次问就可能出现「开关是新那一趟的、圈
                    // 还是上一张的」，而那一排芯片亮哪一枚问的正是这两格。
                    (i32::from(m.ink.outline_colour()[0]) << 16)
                        | (i32::from(m.ink.outline_colour()[1]) << 8)
                        | i32::from(m.ink.outline_colour()[2]),
                    m.ink.outline_width() as i32,
                    m.ink.font_family().to_string(),
                    // 框里预先放着的那一串（§5.7.11 新的一枚是空串，§5.7.16 步 3 改字那一
                    // 只是对象自己那句话）。与 `pen_family` 同一待遇：`MaskViewData` 是
                    // `Copy` 的几何样本，装不动一串字，所以这一格在同一次锁里直接向层里问。
                    m.ink.typing_text().to_string(),
                    // 框里那一串的字体名，与上面那一格同一条腿：改字时读的是对象自己的
                    // `Style.font_family`，落新框时读的是画笔的。
                    m.ink.typing_font_family().to_string(),
                    // 框里那一串字的颜色，与上面两格同一条腿、同一位作者：改字读对象自己的
                    // `Style.color`，落新框读画笔的。打包成 `0xRRGGBB` 与 `pen_rgb` /
                    // `outline_rgb` 同规（alpha 那一枚字节页上按 1.0 铺，本轮没改那一条）。
                    (i32::from(m.ink.typing_colour()[0]) << 16)
                        | (i32::from(m.ink.typing_colour()[1]) << 8)
                        | i32::from(m.ink.typing_colour()[2]),
                    m.ink.eraser_tool(),
                    m.ink.erase_transparent(),
                    m.ink.selected_count() as i32,
                    m.ink.rotation_degrees(),
                    m.ink.marquee_mode(),
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
                // 那一排色带的两枚读数走的是同一条腿：话与钟。它们与上面那一样本同一次锁，所以
                // 页上重读名单的那一刻不可能读到「话是这一趟的、钟是上一趟的」。
                m.band_last.clone(),
                m.band_revisions as i32,
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
        // §5.3.2 第三步那块高亮框与选区用的是**同一份 `view` 样本**：一次 `press`／`drag`／
        // `release` 之后页上重新读这五格，就不会出现「选区是新拖出来的、高亮框还停在按下
        // 之前那一扇上」。`hover_window` 那一路是每次 hover 现算，两路写的是同一格数据，
        // 而都从同一把锁里的那一份状态出发。
        self.as_mut().set_hover_live(view.hover_live);
        self.as_mut().set_hover_x(view.hover.x);
        self.as_mut().set_hover_y(view.hover.y);
        self.as_mut().set_hover_w(view.hover.w as i32);
        self.as_mut().set_hover_h(view.hover.h as i32);
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
            alpha,
            dashed,
            filled,
            effect_tool,
            effect_intensity,
            effect_caption,
            brush_tool,
            brush_shape,
            arrow_tool,
            arrow_head,
            rounded_tool,
            corner_radius,
            number_tool,
            counter_start,
            auto_renumber,
            zoom_tool,
            zoom_percent,
            zoom_border,
            connection_line,
            text_tool,
            pen_font,
            text_align,
            text_bg,
            text_outline,
            outline_rgb,
            outline_width,
            pen_family,
            text_seed,
            text_family,
            text_rgb,
            eraser_tool,
            erase_transparent,
            picked_count,
            picked_rotation,
            marquee_mode,
        ) = ink;
        self.as_mut().set_overlay_key(QString::from(&*overlay));
        self.as_mut().set_tool(tool);
        self.as_mut().set_objects(objects);
        self.as_mut().set_can_undo(can_undo);
        self.as_mut().set_can_redo(can_redo);
        self.as_mut().set_pen_width(width);
        self.as_mut().set_pen_rgb(rgb);
        self.as_mut().set_pen_alpha(alpha);
        self.as_mut().set_dashed(dashed);
        self.as_mut().set_filled(filled);
        self.as_mut().set_effect_tool(effect_tool);
        self.as_mut().set_effect_intensity(effect_intensity);
        self.as_mut()
            .set_effect_caption(QString::from(&*effect_caption));
        self.as_mut().set_brush_tool(brush_tool);
        self.as_mut().set_brush_shape(brush_shape);
        self.as_mut().set_arrow_tool(arrow_tool);
        self.as_mut().set_arrow_head(arrow_head);
        self.as_mut().set_rounded_tool(rounded_tool);
        self.as_mut().set_corner_radius(corner_radius);
        self.as_mut().set_number_tool(number_tool);
        self.as_mut().set_counter_start(counter_start);
        self.as_mut().set_auto_renumber(auto_renumber);
        self.as_mut().set_zoom_tool(zoom_tool);
        self.as_mut().set_zoom_percent(zoom_percent);
        self.as_mut().set_zoom_border(zoom_border);
        self.as_mut().set_connection_line(connection_line);
        self.as_mut().set_text_tool(text_tool);
        self.as_mut().set_pen_font(pen_font);
        self.as_mut().set_text_align(text_align);
        self.as_mut().set_text_bg(text_bg);
        self.as_mut().set_text_outline(text_outline);
        self.as_mut().set_outline_rgb(outline_rgb);
        self.as_mut().set_outline_width(outline_width);
        self.as_mut().set_pen_family(QString::from(&*pen_family));
        self.as_mut().set_eraser_tool(eraser_tool);
        self.as_mut().set_erase_transparent(erase_transparent);
        self.as_mut().set_picked_count(picked_count);
        self.as_mut().set_picked_rotation(picked_rotation);
        self.as_mut().set_marquee_mode(marquee_mode);
        // 先写字、后亮字段：`onVisibleChanged` 是在亮的那一步里读这一格的（见上面那一格的
        // 注释），反过来写就是「框开了、里面没字、提交那一位说不建条」。
        self.as_mut().set_text_seed(QString::from(&*text_seed));
        self.as_mut()
            .set_text_family(QString::from(&*text_family));
        self.as_mut().set_text_rgb(text_rgb);
        self.as_mut().set_text_editing(view.typing_live);
        self.as_mut().set_text_x(view.typing.x);
        self.as_mut().set_text_y(view.typing.y);
        self.as_mut().set_text_w(view.typing.w as i32);
        self.as_mut().set_text_h(view.typing.h as i32);
        self.as_mut().set_text_font(view.font_dip as i32);
        // 圆角那一枚把手与上面那个文本框读的是**同一份 `view` 样本**，所以拖一次把手不会出现
        // 「框是这一趟的、把手是上一趟的」；页上那一枚点只绑这三格，一个像素也不自己算。
        self.as_mut().set_radius_live(view.radius_live);
        self.as_mut().set_radius_x(view.radius.x);
        self.as_mut().set_radius_y(view.radius.y);
        self.as_mut().set_message(QString::from(&*message));
        self.as_mut().set_landed(QString::from(&*landed));
        self.as_mut().set_reveal_last(QString::from(&*reveal_last));
        self.as_mut().set_pre_notice(QString::from(&*pre_notice));
        self.as_mut()
            .set_history_last(QString::from(&*history_last));
        self.as_mut()
            .set_send_to_last(QString::from(&*send_to_last));
        self.as_mut()
            .set_band_last(QString::from(&*band_last));
        self.as_mut().set_band_revision(band_revision);
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

    pub fn hold_constraint(self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.ink.set_constraint(on));
    }

    /// §5.7.18 的那枚 Ctrl 与 `holdConstraint` 走同一条腿：页只递"此刻按着没有"这一枚 bool，
    /// 「这一趟拖的是原物还是副本」在 `annotate::Layer` 里判（那里也只读一次）。
    pub fn hold_clone(self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.ink.hold_clone(on));
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

    /// §5.7.16 步 3 的那一枚 F2。开门不改文档，所以这里没有新像素要发布 —— 但
    /// `text_seed`、`text_x/y/w/h` 与 `text_editing` 这六格都是这一次开门的读数，而别的窗
    /// 要等 QML 把 `grabbed()` 递过去才重读（同一份样本，两扇窗各画自己那一片）。
    pub fn edit_text(mut self: Pin<&mut Self>) -> bool {
        let opened = mask::with(|m| m.edit_text());
        self.as_mut().reload();
        opened
    }

    pub fn step_back(mut self: Pin<&mut Self>) -> bool {
        let cleared = mask::with(|m| m.escape());
        self.as_mut().reload();
        cleared
    }

    /// §5.3.9 第一到三步 —— 方向键把选区挪一格，`reload` 把新框刷到屏幕上。
    ///
    /// §5.3.9 **第 4 步**「放大镜同步显示边缘像素」也在这里落地：同一次锁里紧接着问
    /// [`crate::mask::MaskState::nudge_magnified`]，把放大镜重采样到**刚被挪掉的那条边**，再经
    /// [`Self::apply_magnifier`] 一次改齐那十七格。这一趟屏幕上**没有**新的鼠标事件，所以以前
    /// 那一族停在最后一次 hover 的那一颗像素上 —— 用户一像素一像素地对齐，看的却是别处的像素。
    ///
    /// 「哪一条边、那条边现在在哪一列」不在这里，也不在页上：那是
    /// [`falcon_core::geometry::nudge_sample`] 一件一次算好的事，页上只绑它交出来的那十七格。
    ///
    /// 没有选区可挪的那一趟（`None`）**不动**这一族：放大镜留在它此刻那一颗，既不闪也不藏 ——
    /// 一次谁也没挪成的按键不该让屏幕上那块面板改一次状态。
    pub fn nudge(mut self: Pin<&mut Self>, dx: i32, dy: i32, edge: bool) {
        let index = *self.index();
        let look = mask::MagnifierLook::from_appearance(&settings::appearance_cfg());
        let data = mask::with(|m| m.nudge_magnified(index.max(0) as usize, dx, dy, edge, &look));
        self.as_mut().reload();
        if let Some(data) = data {
            self.as_mut().apply_magnifier(&data);
        }
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

    /// §5.7.2 step 2's 颜色: the three channels the chip carries, and the fourth byte
    /// left where the wheel put it. Replacing it here would make 透明度 un-settable for
    /// anyone who picks another colour afterwards, and the chip's list has no alpha to
    /// offer - it is the palette's `RRGGBB`, so it never could.
    pub fn set_color(mut self: Pin<&mut Self>, r: i32, g: i32, b: i32) {
        let rgb = [
            r.clamp(0, 255) as u8,
            g.clamp(0, 255) as u8,
            b.clamp(0, 255) as u8,
        ];
        mask::with(|m| {
            let rgba = [rgb[0], rgb[1], rgb[2], m.ink.color()[3]];
            m.ink.set_color(rgba);
        });
        self.as_mut().reload();
    }

    /// §5.7.21 step 3's wheel over the colour band. `notches` is the direction and the
    /// count only: the rung size and the two ends of the ladder are
    /// [`crate::annotate::Layer::step_alpha`]'s, and a negative number reaching the u32
    /// behind it by way of `as u32` is the cast `apply_zoom_percent` refuses for.
    pub fn step_alpha(mut self: Pin<&mut Self>, notches: i32) {
        mask::with(|m| m.ink.step_alpha(notches));
        self.as_mut().reload();
    }

    pub fn apply_effect_intensity(mut self: Pin<&mut Self>, value: i32) {
        mask::with(|m| m.ink.set_effect_intensity(value.max(0) as u32));
        self.as_mut().reload();
    }

    /// §5.7.8 规则那一枚「圆形或椭圆形」与 §5.7.9 / §5.7.10 步骤 2 的形状，三档之一。序号
    /// 出格由 `annotate` 那一头**不动笔**接住，所以这一层不预检查：过桥的那一枚钮与文档里那
    /// 一枚笔头是同一位作者（[`Self::apply_arrow_head`] 立的正是这一条）。
    pub fn apply_brush_shape(mut self: Pin<&mut Self>, code: i32) {
        mask::with(|m| m.ink.set_brush_shape(code));
        self.as_mut().reload();
    }

    /// §5.7.6 step 2 的那三档箭头样式。序号出格由 `annotate` 那一头**不动笔**接住，所以这一层
    /// 不预检查：过桥的那一枚钮与文档里的那一枚样式是同一位作者算出来的。
    pub fn apply_arrow_head(mut self: Pin<&mut Self>, code: i32) {
        mask::with(|m| m.ink.set_arrow_head(code));
        self.as_mut().reload();
    }

    /// §5.7.3 step 2 的圆角阶梯：这里只递方向，一端有多少格、两端各是多少都在 `annotate`
    /// ——屏上那一个读数与落进文档的那一个数是同一次 `reload` 读回来的。
    pub fn step_corner_radius(mut self: Pin<&mut Self>, notches: i32) {
        mask::with(|m| m.ink.step_corner_radius(notches));
        self.as_mut().reload();
    }

    /// §5.7.12 step 2 的起始编号阶梯：递方向，两端与格宽在 `annotate`。改完要 `reload`，
    /// 因为屏上那一格读的是计数器此刻的值，而不是这一手刚递出去的那一个。
    pub fn step_counter_start(mut self: Pin<&mut Self>, notches: i32) {
        mask::with(|m| m.ink.step_counter_start(notches));
        self.as_mut().reload();
    }

    /// §5.7.12 step 5 的那一枚开关。它亮与灭改的是**下一次删除怎么被记**，不是屏幕上已有的
    /// 一枚，所以这一手不落任何编辑（[`crate::annotate::Layer::set_auto_renumber`] 头顶那句），
    /// `reload` 要的只是把面板上那一枚钮亮着的读数刷回这一格。
    pub fn apply_auto_renumber(mut self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.ink.set_auto_renumber(on));
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

    /// §5.7.11 step 4's 字号. `set_font_size` takes the same `max(0)` that
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

    /// §5.7.11 step 4's 字体。页上那一格送进来的一串字原样递过去：trim 与「空白不算一枚
    /// 字体」都归 [`crate::annotate::Layer::set_font_family`]，这里不夹第二遍（#281）；
    /// `reload` 才是这一门的落点 —— 交出去之后读回来的那一句可以不是打进去的那一句，所以
    /// 那一格提交后把自己清空、让绑着 `pen_family` 的占位句重新显形（页上不另存一份族名，
    /// 与 `sizeBar` 记过的那一族同一规：`text` 一旦被赋值就不再是绑定）。
    pub fn set_family(mut self: Pin<&mut Self>, family: &QString) {
        mask::with(|m| m.ink.set_font_family(&family.to_string()));
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

    /// §5.7.11 那句规则的颜色那一半。三枚各夹一次 0..=255，与 [`Self::set_color`] 同一条式子：
    /// 色块递来的是 JS 的数，而 `as u8` 会把 -1 变成 255 —— 一枚说"黑"的芯片画出白，是这一格
    /// 唯一不该有的那种错法。那一圈边的 alpha 由 [`crate::annotate::Layer::set_outline_colour`]
    /// 保住原样，这里不递第四枚。
    pub fn set_outline_colour(mut self: Pin<&mut Self>, r: i32, g: i32, b: i32) {
        let rgb = [
            r.clamp(0, 255) as u8,
            g.clamp(0, 255) as u8,
            b.clamp(0, 255) as u8,
        ];
        mask::with(|m| m.ink.set_outline_colour(rgb));
        self.as_mut().reload();
    }

    /// 宽度那一半。这里只把负数拦在 0 之外（`as u32` 会把 -1 变成顶格那枚），阶梯的地板与
    /// 天花板归 core（`Style::set_outline_width` 走笔那把 `WIDTH_MIN..=WIDTH_MAX`，一圈边就是
    /// 一条线），页上不夹第二遍。
    pub fn apply_outline_width(mut self: Pin<&mut Self>, px: i32) {
        mask::with(|m| m.ink.set_outline_width(px.max(0) as u32));
        self.as_mut().reload();
    }

    /// §5.7.14's 擦除到透明. The reload is the whole of it on this side: the mode moves
    /// the pen, and the row the user just clicked reads both the switch and whose tool it
    /// is off the readback rather than off the click.
    pub fn apply_erase_transparent(mut self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.ink.set_erase_transparent(on));
        self.as_mut().reload();
    }

    /// §5.7.17 step 4 的「删除」。返回值只说"这回真删掉了东西"，话仍走 [`Self::message`] 那一族
    /// 之外的路：删掉之后 `reload` 把 `picked_count` 读回 0，那一行自己暗下去，界上不猜。
    pub fn delete_picked(mut self: Pin<&mut Self>) -> bool {
        let done = mask::with(|m| m.ink.delete_selected());
        self.as_mut().reload();
        done
    }

    /// §5.7.18 steps 2-4 的「复制」。副本接着成为这一串，所以读回来的 `picked_count` 还是那么
    /// 多枚 —— 换掉的只是握在手里的是哪几枚，那一行不该在复制之后自己暗下去。
    pub fn clone_picked(mut self: Pin<&mut Self>) -> bool {
        let done = mask::with(|m| m.ink.clone_selected());
        self.as_mut().reload();
        done
    }

    /// §5.7.15 step 2 的角度：QML 只递方向，落成的那个数由 `reload` 从文档里读回来
    /// （[`Self::picked_rotation`]）。页上不自己加 15 —— 那是第二把阶梯，也正是
    /// [`crate::annotate::Layer::step_alpha`] 头上那句话讲的同一件事。
    pub fn step_rotation(mut self: Pin<&mut Self>, notches: i32) -> bool {
        let done = mask::with(|m| m.ink.step_rotation(notches));
        self.as_mut().reload();
        done
    }

    /// §5.7.17 step 1 的框选档：按下去要 `reload`，因为那一枚钮亮不亮读的就是这一格。
    pub fn apply_marquee(mut self: Pin<&mut Self>, on: bool) {
        mask::with(|m| m.ink.set_marquee(on));
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

    /// §5.7.21 第二步的 Rust 侧。这一手**不数格、不排序、不写档**：那三件事只有设置页那两只
    /// 钮在用的 [`crate::settings_view::pal_move`] 一只主人（#281 的口径），这里只是把页上拖出的
    /// 那个差递给它，然后把那句话与那一格钟放进 [`crate::mask::MaskState`]，最后 `reload`。
    ///
    /// 落盘走的是**本机那一台** `Settings`（与设置页同一份档），所以拖一次色块就是一次真改动，
    /// 不是这一场流程里的临时值 —— 这也是 `band_last` 要单独占一格的理由：拒绝得留得下来。
    pub fn move_color(mut self: Pin<&mut Self>, at: i32, delta: i32) -> QString {
        let said = crate::settings_view::pal_move(at, delta).text();
        mask::with(|m| {
            m.band_last = said.clone();
            m.band_revisions += 1;
        });
        self.as_mut().reload();
        QString::from(&*said)
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
        // 钉成了才重读名册 —— 与 `pin_clipboard`、`history_pin` 和热键那一路 `pin_pictures`
        // 同一条腿（规矩与理由写在那一格的注释里，本条不重述）。
        if done {
            crate::hotkey::republish_roster();
        }
        done
    }

    /// §5.8.1 步 2 的那一枚「贴图」。这一枚门把手同样什么都不判：画面、落点、话，全在
    /// [`crate::mask::MaskState::pin_selection`] 那一格，页上只负责把这枚钮按下去。
    /// 名册那一腿与 [`Self::save_and_pin`] 同一条理由、同一只手（`republish_roster`）。
    pub fn pin_selection(mut self: Pin<&mut Self>) -> bool {
        let done = mask::with(|m| m.pin_selection());
        self.as_mut().reload();
        if done {
            crate::hotkey::republish_roster();
        }
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

    /// §5.5.9 的替代分支。这一枚门把手自己什么都**不判**：落盘、命名、撞名、话，全在
    /// [`crate::mask::MaskState::share_out`] 那一格，而它落盘那一步又是
    /// [`crate::mask::MaskState::quick_out`] 的 —— 于是页上按一次「分享」写的文件与按一次
    /// 「快速保存」写的文件出自同一条命名规则，两枚钮不会各自漂移。
    /// `reload` 与 [`Self::quick_save`] 同一条理由：话在那一枚共用的 mask 状态里。
    pub fn share_out(mut self: Pin<&mut Self>) -> bool {
        let done = mask::with(|m| m.share_out());
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

    /// §5.4.1 —— 放大镜那一族的**唯一作者**。一次锁里问 [`crate::mask::MaskState::magnifier_at`]，
    /// 一次锁里把 [`crate::mask::MagnifierData`] 十七格原样抄到 qproperty 上（同 [`Self::reload`]
    /// 里"一次锁一次样本"的口径，只是这一族字段少、代价小，能挂到 hover 每一帧上）。
    ///
    /// **不调 [`Self::reload`]**：reload 每帧要问一整份 [`crate::mask::MaskViewData`] 与
    /// 二十来格 ink，指针走一万下就把那一族负担压到 hover 上；放大镜只按值改自己那十七格，
    /// 别的字段该由 `pressAt`/`dragTo`/`releaseAt`/`commit` 那几手各自负责。
    ///
    /// §5.19.3 那四枚外观**每一趟都问一次** [`crate::settings::appearance_cfg`]，而不是像
    /// [`crate::mask::MaskState::capture`] 那样在 `open()` 里取一份快照：那四枚说的是一扇面板
    /// 的长相、不是这一帧像素从哪来，所以档里改了（设置页写回、§5.20.3 导入）下一次移动鼠标
    /// 就该换脸，不必等下一场遮罩、更不必重启。两把锁**顺序拿、不嵌套** —— `settings` 那一份
    /// 先 clone 出来、锁先关掉，再进 `mask` 的锁，这一族自此不给全进程添一条新的锁序。
    pub fn update_magnifier(mut self: Pin<&mut Self>, x: f64, y: f64) {
        let index = *self.index();
        let look = mask::MagnifierLook::from_appearance(&settings::appearance_cfg());
        let data = mask::with(|m| m.magnifier_at(index.max(0) as usize, x, y, &look));
        self.as_mut().apply_magnifier(&data);
    }

    /// [`crate::mask::MagnifierData`] → 页上绑的那十七格，一次抄齐。**这一族只有这一处**
    /// 写这些 qproperty：鼠标那三处经 [`Self::update_magnifier`] 走进来，§5.3.9 第 4 步键盘
    /// 那一趟经 [`Self::nudge`] 走进来。两处各抄一遍的话，第二遍漏掉哪一格（比如新加的
    /// `magnifier_anchor_x`）都不会报错，只会让那一格跟着旧值停在屏幕上。
    ///
    /// 不收 `Pin` 之外的返回：这一族按值改属性，QML 侧绑的是属性本身（§3.6 约束 8 那一族，
    /// 与 `reload` 同一口径）。
    fn apply_magnifier(mut self: Pin<&mut Self>, data: &mask::MagnifierData) {
        self.as_mut().set_magnifier_show(data.show);
        self.as_mut().set_magnifier_key(QString::from(&*data.key));
        self.as_mut().set_magnifier_w(data.w);
        self.as_mut().set_magnifier_h(data.h);
        self.as_mut().set_magnifier_coord(QString::from(&*data.coord));
        self.as_mut().set_magnifier_color(QString::from(&*data.color));
        self.as_mut().set_magnifier_rgb(data.rgb);
        self.as_mut().set_magnifier_has_color(data.has_color);
        self.as_mut().set_magnifier_grid(data.grid);
        self.as_mut().set_magnifier_pixel_w(data.pixel_w);
        self.as_mut().set_magnifier_zoom(data.zoom);
        self.as_mut().set_magnifier_anchor_x(data.anchor_x);
        self.as_mut().set_magnifier_anchor_y(data.anchor_y);
        self.as_mut()
            .set_magnifier_background(QString::from(&*data.look.background));
        self.as_mut()
            .set_magnifier_foreground(QString::from(&*data.look.foreground));
        self.as_mut().set_magnifier_border(data.look.border);
        self.as_mut().set_magnifier_corner_radius(data.look.radius);
    }

    /// §5.3.2 第二、三步。一次 [`crate::mask::MaskState::hover_window`] 之后紧接着把
    /// **同一把锁里**的窗内读数交出去 —— 分成两趟锁就会读到「命中是这一趟、框是上一趟」。
    ///
    /// 越界的 `index` 走 `max(0)` 与 [`Self::hit_test`] 同一写法：这一格错了是 QML 的一次
    /// 编程事故，它不该让桥上任何一列改动，而 `hover_window` 自己答 `false`。
    pub fn hover_window(mut self: Pin<&mut Self>, x: f64, y: f64) -> bool {
        let index = *self.index();
        let (live, rect) = mask::with(|m| {
            let index = index.max(0) as usize;
            let live = m.hover_window(index, x, y);
            (live, m.hover_bounds(index).unwrap_or_default())
        });
        self.as_mut().set_hover_live(live);
        self.as_mut().set_hover_x(rect.x);
        self.as_mut().set_hover_y(rect.y);
        self.as_mut().set_hover_w(rect.w as i32);
        self.as_mut().set_hover_h(rect.h as i32);
        live
    }

    /// §5.4.2 —— 裸 `C`。走 [`crate::mask::MaskState::pick_color`]，之后**只**刷 `message`
    /// 一列：这一手不改选区、不改孔、不改放大纹理，一次 `reload` 的十页数据里只有那一条
    /// 是新的，其余全是同一把锁里同一份样本 —— 拿一份现成的重问一遍不省什么，白搭一次锁。
    pub fn pick_color(mut self: Pin<&mut Self>) -> bool {
        let message = mask::with(|m| {
            let done = m.pick_color();
            (done, m.message.clone())
        });
        self.as_mut()
            .set_message(QString::from(&*message.1));
        message.0
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

// §5.3.9 第 4 步「键盘微调选区时，放大镜应同步显示边缘像素」的**接线**判据。
//
// 那颗点本身住在 `falcon_core::geometry::nudge_sample`（纯函数，cargo 在那一档判四枚箭头 /
// 夹在桌面边上 / Alt 只挪一条边 / 零位移交回指针），桥交出的那一份读数住在 `crate::mask` 的
// 单测里。这一档判的是**剩下那一段**：页上方向键按下去，那一趟真的走到桥；桥那一趟真的叫了
// 放大镜的装配；装配的结果真的写进页上绑的那几格。**函数存在 ≠ 有人叫它** ——「逻辑齐备、
// 生产路径零调用」那一族（#269）。
//
// 尺子与全档同一把：`include_str!` 拿的就是**编译进这一手的那一份源**，QML 那一面先经
// `hint_check::qml_code`（全档唯一一把剥注释的尺子）过一遍，所以一句写在注释里的散文
// 过不了这一格；Rust 那一面按**函数本体**切，"这一档某处出现过那个名字"不算接线。
#[cfg(test)]
mod tests {
    /// 本档自己：`nudge` 那一手与 `apply_magnifier` 那一处装配。
    const VIEW_SRC: &str = include_str!("mask_view.rs");
    /// 状态那一头：`nudge_by` / `nudge_magnified` / 那唯一一次放大镜装配。
    const MASK_SRC: &str = include_str!("mask.rs");
    /// 页那一头：方向键、`step()`、面板摆在哪一格绑的是谁。
    const PAGE: &str = include_str!("../qml/CaptureMask.qml");

    /// 取一枚函数**从签名到收尾那枚大括号**之间的那一段。整档 `contains` 只能答"这一档某处
    /// 叫过它"，而接线要读的是"**这一手里**叫了它"。签名找不到、或找不到收尾就 panic ——
    /// 判据不许在一集上通过。
    fn fn_body(src: &str, head: &str) -> String {
        let start = src
            .find(head)
            .unwrap_or_else(|| panic!("这一档里找不到 {head}，那把尺子今天量的是空集"));
        let tail = src[start..]
            .find("\n    }")
            .unwrap_or_else(|| panic!("{head} 之后找不到四格缩进的那枚收尾大括号"));
        src[start..start + tail].to_string()
    }

    /// 把一枚"针"拼出来。数**出现次数**的那种判据（"写这一格的只有一个人"）不能把针写成整串
    /// 字面：`include_str!` 拿的是整档，本测试模块里那一句字面也算这一档的一处出现。
    fn joined(head: &str, tail: &str) -> String {
        format!("{head}{tail}")
    }

    /// 页上那四枚方向键都走 `win.step()`，那一手叫桥的 `nudgeHole`，并且把新框广播给别的屏；
    /// 而放大镜面板摆在哪一格绑的是**桥交出的那两格**，页上不再自己记一份"指针在哪"。
    #[test]
    fn the_pages_keyboard_arm_calls_the_bridge_and_binds_the_anchors_from_it() {
        let code = crate::hint_check::qml_code(PAGE);
        assert!(
            code.contains("view.nudgeHole("),
            "剥注释之后页上连方向键那条腿都不在了 ⇒ 这把尺子今天量的是空集"
        );
        for key in ["Left", "Right", "Up", "Down"] {
            assert!(
                code.contains(&format!("Keys.on{key}Pressed: (event) => win.step(")),
                "方向键 {key} 没走 `win.step(` 那一手 —— 四枚各写一份就会有一份漏掉同步"
            );
        }
        // 面板摆在哪一格：绑桥那一趟真的去采的那一点（#281 —— 页上那份副本就是这一轮删掉的）。
        assert!(
            code.contains("win.view.magnifier_anchor_x"),
            "放大镜面板的位置没绑桥交出的那一点"
        );
        assert!(
            code.contains("win.view.magnifier_anchor_y"),
            "放大镜面板的位置只绑了横格、没绑竖格"
        );
        assert!(
            !code.contains("win.cursorX") && !code.contains("win.cursorY"),
            "页上还留着一份自己记的指针位置 —— 那正是「键盘把框挪走了、面板还在原地」的第二个作者"
        );
        // 键盘那一趟也要发 `grabbed()`：选区是一整个桌面矩形，别的屏上那块暗区得跟着挪。
        let step = fn_body(&code, "function step(");
        assert!(
            step.contains("view.nudgeHole(") && step.contains("win.grabbed()"),
            "`step()` 那一手里要么没叫桥、要么没把新框广播给别的屏"
        );
    }

    /// 桥里那一手 `nudge` 之后**紧接着**问放大镜的装配，并把结果写进页上绑的那几格；`None`
    /// 那一趟什么都不动。这一格读的是那一手的**本体**，不是"档里有这么个名字"。
    #[test]
    fn the_bridges_nudge_leg_re_runs_the_magnifier_in_the_same_step() {
        let nudge = fn_body(VIEW_SRC, "pub fn nudge(");
        assert!(
            nudge.contains("m.nudge_magnified("),
            "键盘那一趟没去问放大镜该采哪一颗 —— §5.3.9 第 4 步落空"
        );
        assert!(
            nudge.contains("apply_magnifier(&data)"),
            "问到了读数却没写进页上绑的那几格"
        );
        assert!(nudge.contains("reload()"), "选区本身要刷到屏幕上（§5.3.9 第一到三步）");
        assert!(
            nudge.contains("if let Some(data) = data"),
            "没有选区那一趟被当成「清掉放大镜」：按一次空方向键面板就闪一下"
        );
        // 那两格必须先在桥的那一副面孔上声明过，页上绑的才不是 `undefined`。
        assert!(
            VIEW_SRC.contains("#[qproperty(f64, magnifier_anchor_x)]")
                && VIEW_SRC.contains("#[qproperty(f64, magnifier_anchor_y)]"),
            "面板落点那两格没上桥的面孔，页上绑的是不存在的属性"
        );
        // 两条腿共用**同一处**写这一族的人：各抄一遍时，漏掉那一格不会报错，只会停在旧值上。
        assert!(
            fn_body(VIEW_SRC, "pub fn update_magnifier(").contains("apply_magnifier(&data)"),
            "鼠标那一腿不走同一处装配，两条腿就会各有一份读数"
        );
        assert_eq!(
            fn_body(VIEW_SRC, "fn apply_magnifier(")
                .matches("set_magnifier_anchor")
                .count(),
            2,
            "那两格里有一格没被写 —— 键盘那一趟面板就会停在旧的那一格"
        );
        // 数"写这一格的只有一个人"那一枚针要**拼**出来：`include_str!` 拿的是整档，而本测试
        // 模块里那句字面也是这一档里的一处出现 —— 把自己数进去，这一格就永远红。
        let writer = joined("set_", "magnifier_anchor_x(");
        assert_eq!(
            VIEW_SRC.matches(writer.as_str()).count(),
            1,
            "第二处写面板落点的人：同一件事又有了两个作者"
        );
    }

    /// 状态那一头：`nudge_by` 交出**刚被挪掉的那条边**，`nudge_magnified` 把同一次挪与那颗点
    /// 递给鼠标那条腿走的**同一次装配**；"放大镜此刻在采哪一颗"全档只有一个写它的人。
    #[test]
    fn the_states_keyboard_leg_hands_the_moved_edge_to_the_one_assembly() {
        let by = fn_body(MASK_SRC, "pub fn nudge_by(");
        assert!(
            by.contains("nudge_sample("),
            "挪完之后没算那颗边 —— 放大镜无从同步"
        );
        assert!(by.contains("set_hole("), "连选区都没挪");
        assert!(
            !by.contains("store_raw"),
            "这一手只管几何：像素那一段该在唯一那次装配里，别在这儿再发一遍纹理"
        );
        let via = fn_body(MASK_SRC, "pub fn nudge_magnified(");
        assert!(
            via.contains("self.magnifier_at_point("),
            "键盘那一腿另走了一次装配 = 给同一块面板找了第二个作者"
        );
        assert!(
            fn_body(MASK_SRC, "pub fn magnifier_at(").contains("self.magnifier_at_point("),
            "鼠标那一腿也该走那**同一次**装配"
        );
        assert_eq!(
            MASK_SRC.matches("self.cursor_phys = Some(").count(),
            1,
            "第二处记「放大镜此刻在采哪一颗」的人：按 C 复制的与面板说的就会分家"
        );
    }

    /// §5.9.17 步 1 那枚开关（「复制图片时同时提供文件」）在三只**正在复制图片的手**上的接线：
    /// 每一只都在自己的本体里读那一枚键，开着的每一只都只经由 `state` 那一架梯子递出去，关着的
    /// 每一只都还留着 §5.9.16 那句逐字未改的话。
    ///
    /// 为什么是源码扫描而不是跑一次 `copy_out()`：跑一次就要写用户的剪贴板（§10 第 51 项 ② 立的
    /// 正是这一枚牌 —— 单元档在锁屏会话里也照跑），而这一格要读的是「分岔在哪一只手、由谁决定」，
    /// 那一问在源里就答得完。真递出去那一路的读数在 `--annotate` 的 `clipboard file ref` 那一格与
    /// `--pins` 的 `pin copy file` 那一格里，两档都要用户当场点一次头。
    #[test]
    fn every_door_that_copies_a_picture_dispatches_on_the_file_switch() {
        const SWITCH: &str = "crate::settings::output_cfg().copy_as_file";
        const SESSION_SRC: &str = include_str!("session.rs");

        for (src, head, door) in [
            (MASK_SRC, "pub fn copy_out(", "ClipDoor::Selection"),
            (MASK_SRC, "fn auto_copy_out(", "ClipDoor::AutoSelection"),
        ] {
            let body = fn_body(src, head);
            assert!(
                body.contains("clip::usable()"),
                "{head} 那三道梯被拆了：占用者的名字要在动手之前就报出来（§7.2）"
            );
            assert_eq!(
                body.matches(SWITCH).count(),
                1,
                "{head} 那一手里读那枚开关的次数不是 1：读两次就是允许它在同一趟里改主意"
            );
            assert!(
                body.contains(&format!("copy_frame_with_files(&frame, crate::state::{door})")),
                "{head} 开着那一侧没走 state 那一架梯子 —— §5.9.17 那两条规则只在梯子里有作者"
            );
            assert!(
                body.contains("platform_windows::clip::write_image(&frame)"),
                "{head} 关着那一侧的话被改了：§5.9.16 的读法原本就是这一句"
            );
        }

        // 反证（#274）：两扇门各拿自己那一枚 `ClipDoor`，谁也不许借走另一扇的话 —— 遮罩那一扇是
        // 有人按的，听到「默认行为没递东西」就不知道自己那一次按失败了；默认行为那一扇没人按过，
        // 听到「可以再复制一次」就是替用户安排了一次他没做的手指。
        assert!(
            !fn_body(MASK_SRC, "pub fn copy_out(").contains("AutoSelection"),
            "有人按的那一扇借走了默认行为那句话"
        );
        assert!(
            !fn_body(MASK_SRC, "fn auto_copy_out(").contains("ClipDoor::Selection"),
            "默认行为那一扇借走了有人按那句话"
        );

        // 门不自己造第二架梯子：`CF_HDROP` 那一手在整档 mask.rs 里一次都不许出现。针要拼出来 ——
        // `include_str!` 拿的是整档，本测试模块里那句字面也算这一档的一处出现。
        let needle = joined("clip::write_image_with_files", "(");
        assert_eq!(
            MASK_SRC.matches(needle.as_str()).count(),
            0,
            "遮罩那一头自己递了文件列表：§5.9.17 那两条规则就有了第二个作者（#281）"
        );

        // 第三扇门在 session.rs 那一头（贴图菜单的「复制图片」与贴图上的 `Ctrl+C`）。它早由
        // `pin_check::copy_rows` 在真剪贴板上判过，这一格只钉一句：读的也是同一枚键、递的也是
        // 同一架梯子，所以三扇门不会各有一份「开关此刻是开着还是关着」的读数。
        let copy_pin = fn_body(SESSION_SRC, "pub fn copy_pin(");
        assert_eq!(
            copy_pin.matches(SWITCH).count(),
            1,
            "贴图那一扇读那枚开关的次数不是 1"
        );
        assert!(
            copy_pin.contains("s.copy_image_with_files(id as u64)"),
            "贴图那一扇没走同一架梯子：两条腿就会各判各的「剪贴板此刻点名了没有」"
        );
    }

    /// §5.7.16 步 3「文字内容」那一只手在屏幕上的整条腿：裸 F2 → `view.editText()` →
    /// `MaskState::edit_text` → `Layer::edit_selected_text`，以及框里预先放着的那一串与它
    /// 那枚字体名怎么从桥走回页上。
    ///
    /// 四手的**读数**各在自己那一档判过（`annotate::tests` 判门与那两条梯子，`mask::tests`
    /// 判除法与那句话），这一档只钉名字对得上：函数存在而没人叫它，是「逻辑齐备、生产路径
    /// 零调用」那一族（#269）。
    ///
    /// 真按键那一路这一轮没有证人：这一档不能合成一次按键（`CaptureMask.qml` 里 `chord()`
    /// 头上记的正是这条理由 —— 注入的键会落进当时有焦点的那个应用），而跑级时桌面在用户手里。
    /// 页上那一条 handler 到 `editText` 那一步因此是**源码扫描**判的，不是判到的行为。
    #[test]
    fn the_f2_door_walks_from_the_page_to_the_layer_and_back_as_two_strings() {
        const ANNOTATE_SRC: &str = include_str!("annotate.rs");
        const BAR: &str = include_str!("../qml/AnnotationToolbar.qml");

        // ① 页上那一枚键：不带任何修饰键才算它（与裸 C 同一规），开了就把这一趟递给别的窗。
        let code = crate::hint_check::qml_code(PAGE);
        let at = code
            .find("&& event.key === Qt.Key_F2) {")
            .unwrap_or_else(|| panic!("剥注释之后页上连 F2 那条腿都不在了 ⇒ 这把尺子今天量的是空集"));
        // 那一手自己的胳膊（到 `return` 为止）：开了门才广播，而判据一条都不在页上重写。
        let arm = &code[at..code[at..].find("return").map_or(code.len(), |k| at + k)];
        assert!(
            arm.contains("if (view.editText()) {") && arm.contains("win.grabbed()"),
            "F2 那一手要么没叫桥、要么开了门没把这一趟递给别的屏：那只字段该在哪一扇亮就成了上一趟的读数"
        );
        assert!(
            !arm.contains("selectedCount") && !arm.contains("textEditing"),
            "页上自己数了一遍「哪一枚改得」—— 同一件事就有了两个作者（#281）"
        );

        // ② 桥的那一手指：`cxx_name` 与页上叫的那个名字必须成对，否则 QML 那边叫的是
        // 一个不存在的属性，而 qmlcachegen 不会替这一格报错。
        let face = VIEW_SRC.replace('\r', "");
        assert!(
            face.contains("#[cxx_name = \"editText\"]\n        fn edit_text(self: Pin<&mut Self>) -> bool;"),
            "桥的那张面孔上 `editText` 与 `edit_text` 不成对 —— 页上那一手叫的是不存在的属性"
        );
        let door = fn_body(VIEW_SRC, "pub fn edit_text(");
        assert!(
            door.contains("mask::with(|m| m.edit_text())") && door.contains("reload()"),
            "那一手要么没递给状态那一头、要么刷完没把六格重读（开了框屏幕上没有字段）"
        );

        // ③ 状态那一头：门本身不判据，只递问题；没开门时屏幕上要有话（#433）。
        let state = fn_body(MASK_SRC, "pub fn edit_text(");
        assert!(
            state.contains("self.ink.edit_selected_text()"),
            "状态那一头没把这一问递给画图的那只手"
        );
        assert!(
            state.contains("self.message = "),
            "拒门没有话要说：按下去没反应的四种可能，屏幕上一种也不分辨"
        );

        // ④ 层那一头：四道闸各是各的，且在**开门之前**问。
        let layer = fn_body(ANNOTATE_SRC, "pub fn edit_selected_text(");
        for gate in [
            "self.tool.is_some() || self.typing.is_some()",
            "let [id] = self.sel.ids()",
            "e.kind.takes_text()",
            "self.doc.can_edit(id)",
        ] {
            assert!(layer.contains(gate), "层那一头少了一道闸：{gate}");
        }

        // ⑤ 回来的两串：页上那枚字段读的是**框**的那两格，而工具栏那一排读的还是画笔。
        assert!(
            code.contains("field.text = win.view.text_seed"),
            "框打开时没读桥交出的那一串：改字那一臂打开的是一只空框，交回的就是「不建条」"
        );
        assert!(
            code.contains("font.family: win.view.text_family"),
            "框里的字体绑回了画笔的那一枚：改字时页上 preview 的是对象没有的那一族"
        );
        assert!(
            !BAR.contains("text_family") && BAR.contains("bar.view.pen_family"),
            "工具栏那一排的字体名换了人：亮着的该是画笔此刻带的那一族"
        );

        // ⑥ 两串都必须在 `text_editing` **之前**写下：页上那一格是在可见性翻过去那一刻
        // imperative 地读它们的（`onVisibleChanged`），反过来写就是「框开了、里面没字」。
        let reload = fn_body(VIEW_SRC, "pub fn reload(");
        for seed in ["set_text_seed", "set_text_family"] {
            let at = reload
                .find(seed)
                .unwrap_or_else(|| panic!("reload 那一手里没人写 {seed} —— 页上读的是旧的那一格"));
            let lit = reload
                .find("set_text_editing")
                .expect("reload 那一手里没人亮那枚字段");
            assert!(at < lit, "{seed} 写在 `set_text_editing` 之后：字段亮的那一刻读到的还是空串");
        }

        // ⑦ 那两格得先在桥的那张面孔上声明过、也得在 `MaskViewRust` 里有个家，否则
        // `set_*` 根本不存在（这一轮编译就是这么第一次发现的）。
        for name in ["text_seed", "text_family"] {
            assert!(
                VIEW_SRC.contains(&format!("#[qproperty(QString, {name})]")),
                "{name} 没上桥的面孔，页上绑的是不存在的属性"
            );
            assert!(
                VIEW_SRC.contains(&format!("{name}: QString,")),
                "{name} 在 `MaskViewRust` 里没有那一格"
            );
        }

        // ⑦′ 同一只框的第三格读数：颜色。页上那一行 `color:` 过去解的是 `pen_rgb`，而提交上去
        // 的那一枚保留对象自己的那一枚 —— 字会在框关掉那一刻换一次色。它是**声明式**绑定，
        // 所以不进 ⑥ 那一支顺序（写在 `text_editing` 前后都读得到）；上面孔、有那一格、有人写、
        // 只有一位作者这四条照旧。
        assert!(
            code.contains("(win.view.text_rgb & 255) / 255"),
            "字段的颜色没绑框的那一格：改字时页上 preview 的是对象没有的那一枚颜色"
        );
        assert!(
            !code.contains("(win.view.pen_rgb & 255) / 255"),
            "字段还在解画笔的那一枚：同一件事两格读到一个不是它的东西"
        );
        assert!(
            VIEW_SRC.contains("#[qproperty(i32, text_rgb)]") && VIEW_SRC.contains("text_rgb: i32,"),
            "text_rgb 没上桥的面孔，或在 `MaskViewRust` 里没有那一格"
        );
        assert!(
            reload.contains("set_text_rgb(text_rgb)"),
            "reload 那一手里没人写颜色那一格：页上绑的是构造那一刻的 0"
        );
        assert_eq!(
            ANNOTATE_SRC.matches(&joined("self.box_style().", "color")).count(),
            1,
            "「这一只框是什么颜色」有了第二个作者，或压根没人答它"
        );

        // ⑧ 每位作者一处：框那一头读的是 `typing_text` / `typing_font_family` / `typing_font_px`，
        // 工具栏那一排读的还是画笔的 `font_px` / `font_family`。针要拼出来 —— `include_str!` 拿的
        // 是整档，本测试模块里那句字面也算这一档的一处出现。
        for (producer, file) in [
            (joined("m.ink.typing_", "text().to_string()"), VIEW_SRC),
            (
                joined("m.ink.typing_font_", "family().to_string()"),
                VIEW_SRC,
            ),
            (joined("m.ink.font_", "px() as i32"), VIEW_SRC),
            (joined("m.ink.font_", "family().to_string()"), VIEW_SRC),
            (joined("self.ink.typing_font_", "px()"), MASK_SRC),
        ] {
            assert_eq!(
                file.matches(producer.as_str()).count(),
                1,
                "「{producer}」的出现次数不是 1：同一格有了第二个作者，或压根没人递它"
            );
        }

        // 反证（#274）：字段那一格不许绕过桥、自己算一次框的尺寸 —— 页上算不出 DIP，而
        // `font_dip` 是同一份样本里除过的那一枚。
        assert!(
            code.contains("font.pixelSize: win.view.text_font"),
            "字段没绑桥交出的那一格字号，页上就开始了第二次除法"
        );
    }

    /// §5.7.18 步 2「按住 Ctrl 拖动」那一条腿的名字与次序：页在**这一枚按下之前**写那枚修饰键
    /// → 桥的 `holdClone` → [`crate::annotate::Layer::hold_clone`] → `Layer::press` 读一次并交
    /// 给 `select_press` → 只有"按中了某一枚对象"那一臂走 `clone_press` → 松手由
    /// `select_release` 递给 `finish_clone_drag` 落那一枚 `Command::Add`。
    ///
    /// 各手的**行为**在自己那一档判过：`annotate::tests` 判那五扇（一条手势 = 一条撤销、半路
    /// 丢掉不留副本、把手那一臂不复制、成组一次复制整组），`mask::tests` 判遮罩转交与 DIP→桌面
    /// 那一乘。这一档只钉"函数存在且有人叫它"（#269 那一族）与三处次序，不再判第二遍行为。
    ///
    /// 真按键那一路这一轮没有证人：这一档既不能合成"按着 Ctrl 的另一只手按下左键并拖"，跑级时
    /// 桌面又在用户手里（`CaptureMask.qml` 里 `chord()` 头上记的正是注入的键会落进当时有焦点的
    /// 那个应用）。所以页到桥那一跳是**源码扫描**判的，不是判到的行为。
    #[test]
    fn the_ctrl_clone_leg_walks_from_the_page_to_the_layer_in_one_order() {
        const ANNOTATE_SRC: &str = include_str!("annotate.rs");

        // ① 页：这一枚键写在 `pressAt` **之前**（`Layer::press` 读一次即归 false，写晚了这一趟
        // 就成了一次普通移动），而且只在按下这一刻写 —— 它不像 `holdConstraint` 那样每一寸移动
        // 都重写，"中途松手把已经开始的克隆改回移动"是另一条规矩。
        let code = crate::hint_check::qml_code(PAGE);
        assert_eq!(
            code.matches("view.holdClone(").count(),
            1,
            "那一枚键在页上不是一处：写成每一寸移动都重画一遍，就把「中途松手」改成了另一种手势"
        );
        let at = code
            .find("view.holdClone((mouse.modifiers & Qt.ControlModifier) !== 0)")
            .unwrap_or_else(|| panic!("剥注释之后页上连那枚键都不在了 ⇒ 这把尺子今天量的是空集"));
        let pressed = code
            .find("win.grip = view.pressAt(mouse.x, mouse.y)")
            .expect("页上没有那枚按下");
        assert!(at < pressed, "那枚键写在按下之后：`pressAt` 读到的是上一趟的读数");
        assert!(
            code.contains("view.holdConstraint((mouse.modifiers & Qt.ShiftModifier) !== 0)"),
            "两枚键并成了一枚：Shift 一边说「锁 1:1」一边说「拖的是副本」，屏幕上没人猜得对"
        );
        // 反证（#281）：页上不判「这一趟拖的是谁」，也不自己挑对象、自己复制。带 `view.` 前缀
        // 问的是"有没有这样一手调用"，注释里那些名字（`topmost_at` 就在隔壁那一行）不算。
        assert!(
            !code.contains("view.topmost") && !code.contains("view.clonePicked"),
            "页上开始自己挑对象 / 自己复制了：一处判据两个作者"
        );

        // ② 桥的面孔：`cxx_name` 与页上叫的名字不成对，QML 叫的就是一个不存在的属性，而
        // qmlcachegen 不会替这一格报错。
        let face = VIEW_SRC.replace('\r', "");
        assert!(
            face.contains("#[cxx_name = \"holdClone\"]\n        fn hold_clone(self: Pin<&mut Self>, on: bool);"),
            "桥的那张面孔上 `holdClone` 与 `hold_clone` 不成对"
        );
        let door = fn_body(VIEW_SRC, "pub fn hold_clone(");
        assert!(
            door.contains("mask::with(|m| m.ink.hold_clone(on))"),
            "那一手没递给层，或干脆递给遮罩那状态的另一格"
        );
        assert!(
            !door.contains("reload()"),
            "这一格不 publishes 任何东西：跟着刷一遍就是每一寸按下都重读六格"
        );

        // ③ 层：这一枚格子只有两个人碰 —— 写的是界面递进来的那一手，读的是 `press`，而读一次
        // 就用 `mem::replace` 归位（留给下一枚按下就是"键早松了、笔还在克隆"那一扇门）。
        assert_eq!(
            ANNOTATE_SRC.matches("self.clone_hold").count(),
            1 + 1,
            "「这一枚按下按着 Ctrl 吗」的作者不止两个（写它的一手 + 读它的一手）"
        );
        assert!(
            fn_body(ANNOTATE_SRC, "pub fn hold_clone(").contains("self.clone_hold = on;"),
            "层那一头没有那一手，页递进来的 bool 就没地方落"
        );
        let press = fn_body(ANNOTATE_SRC, "pub fn press(");
        assert!(
            press.contains("std::mem::replace(&mut self.clone_hold, false)")
                && press.contains("self.select_press(device, ctrl)"),
            "`press` 没读一次就归位，或没把它递给箭头工具那一手（手里有笔时它照样该被用掉）"
        );

        // ④ 只有"按中了某一枚对象"那一臂复制：握住把手那一臂改尺寸、空白那一臂清那一串，
        // 两臂都从不看 `ctrl`；框选那一档在 `ctrl` 之前就走掉了。
        let select = fn_body(ANNOTATE_SRC, "fn select_press(");
        assert_eq!(
            select.matches("if ctrl").count(),
            1,
            "「按住 Ctrl」在一手里有了两臂：一次按下同时改尺寸又复制一枚"
        );
        assert_eq!(
            select.matches("self.clone_press(").count(),
            1,
            "副本的起手不止一处调用，或一处也没有"
        );
        assert!(
            fn_body(ANNOTATE_SRC, "fn clone_press(")
                .contains(&joined("self.sel.clone_by(&self.doc, ", "0, 0)")),
            "副本起手没走 core 那一位作者，或者给自己偏了位置"
        );

        // ⑤ 落栈与丢弃：松手那一路递给 `finish_clone_drag`，而四条没交出去的路（改档框选、换
        // 工具、点空白、选区被清）都从 `drop_sel_drag` 那一格收预览。计数不钉（#455），钉的是
        // 每一格都在场且被叫到。
        assert!(
            fn_body(ANNOTATE_SRC, "fn select_release(").contains("self.finish_clone_drag(&drag)"),
            "松手没有递给落栈那一手：副本成了永远不进撤销栈的一枚"
        );
        let drop = fn_body(ANNOTATE_SRC, "fn drop_sel_drag(");
        assert!(
            drop.contains("self.take_clone_preview(&drag)"),
            "丢掉那一手没收预览：半途取消的克隆在文档里留下一枚没登记的对象"
        );
        for caller in ["pub fn set_marquee(", "pub fn select_tool(", "fn select_press("] {
            assert!(
                fn_body(ANNOTATE_SRC, caller).contains("self.drop_sel_drag()"),
                "{caller} 那一手没收回预览副本"
            );
        }
    }
}
