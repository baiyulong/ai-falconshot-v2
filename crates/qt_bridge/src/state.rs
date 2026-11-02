//! The pins that are on screen, and the one place that keeps Qt and the state
//! machine in agreement.
//!
//! `falcon_core::pin` owns the semantics and is tested on its own; this module
//! owns the *live session*: the set, the pixels behind each pin, the desktop the
//! pins are clamped to, and which pins Qt has been handed a texture for. Nothing
//! here knows QML exists.
//!
//! The one rule: a pin's picture crosses the boundary only when the pixels
//! change. Zoom and position are pure window geometry - `render_shown`
//! deliberately ignores zoom, so a wheel gesture resizes a window and re-scales a
//! texture in place without touching the encoder. That is what keeps plan rule 4
//! ("the hot path must not cross") true while the wheel is being spun.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Instant;

use falcon_core::config::{AlphaBg, Config, Pin as PinConfig};
use falcon_core::dialog_link::{self, Hidden};
use falcon_core::encode::{self, EncodeOptions, Format};
use falcon_core::frame::Frame;
use falcon_core::geometry::{DipRect, PhysPoint, PhysRect, PhysSize, Scale};
use falcon_core::gif::GifClip;
use falcon_core::naming;
use falcon_core::pin::{
    clamp_zoom, Desktop, GroupFate, GroupId, Monitor, PinId, PinItem, PinSet, PinSlot,
};
use falcon_core::refresh::Refresh;

use crate::capture;
use crate::session::shim;

/// Everything a pin window binds to. Plain fields, because it crosses into a
/// CXX-Qt object one property at a time and has to be readable in a test.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PinViewData {
    /// False once the pin has left the set - the window hides itself. Also false while
    /// a file save dialog is on the screen and this pin was one of 那几张可能遮挡它的
    /// 贴图（§5.5.11 步骤 3）：藏与放回都只改这一格，所以窗口那一侧仍然只有一行绑定。
    /// 自 P135 起还多一臂 false：§5.11 独显把没选中的那些枚藏起来 —— 问的还是
    /// [`falcon_core::pin::PinItem::visible`] 那一格已经答过的答案，桥侧不长第二份。
    pub live: bool,
    pub pos_x: i32,
    pub pos_y: i32,
    pub size_w: i32,
    pub size_h: i32,
    /// The ratio of the monitor this window sits on, so QML can turn the two
    /// physical sizes above into logical ones. It is answered here rather than from
    /// a `Screen` attached object because that object resolves only once the window
    /// is placed, and placing it is what needs the ratio - the cycle behind plan
    /// §10 第 52 项 ①. `0.0` is never a ratio, so a dead pin answers `1.0`.
    pub dpr: f64,
    pub zoom: i32,
    pub opacity: i32,
    pub smooth: bool,
    pub topmost: bool,
    pub click_through: bool,
    /// Bumped only when new pixels went out; QML appends it to the provider URL
    /// so a texture it already cached is re-requested.
    pub revision: u32,
    /// §5.9.14: the four numbers a pin window needs to drive a clip. `gif_frames`
    /// is 0 for a pin that is not a GIF at all, which is what lets one binding
    /// (`running: view.gifPlaying`) be false for every still without a second
    /// question. The per-frame pace is not here because QML does not decide when
    /// a frame is over - [`PinState::tick_gif`] does, from the clip's own list.
    pub gif_frames: i32,
    pub gif_frame: i32,
    pub gif_playing: bool,
    pub gif_speed: i32,
    /// §5.9.15: the mode this pin treats transparent pixels in, as the row number
    /// the menu carries. An index rather than a name because the menu reads it to
    /// check one of four rows, and a name would make that row a string compare in
    /// QML against a spelling that lives in Rust.
    pub alpha_bg: i32,
    /// Whether the picture has a hole in it at all. §5.9.15 步骤 1 opens that menu
    /// from a *透明图片*, so the section has to answer this before offering a mode;
    /// a pin whose picture is fully opaque gets no menu section, not a section that
    /// changes nothing. Scanned on [`PinState::sync`]'s way out rather than on every
    /// reload: this is a fact about the pixels, and a reload happens for gestures
    /// that move no pixel at all.
    pub has_transparent_pixels: bool,
    /// §5.10.1 的那一份「统一选中状态」落到窗上的一格：名单只有一份（core 的
    /// `PinSet::selection`），这里只是**读出**它此刻是否指着这一枚，QML 那一侧不许另存一份
    /// 名单。选中的贴图描一圈边，未选的没有。
    pub selected: bool,
    /// §5.11 的「这一枚是被独显藏起来的，还是被机器压住的」那一问：`live` 那一格把两件事
    /// 合成一枚布尔，而量规要分开问（藏起来的是独显干的，压住的是对话框干的），
    /// §5.11 规则②「恢复原状态而不是无条件显示全部」也只有分开才量得出来。
    /// 读的是 core 那一枚字段本体，不在这里重新判断（#281）。
    pub solo_hidden: bool,
    /// §5.9.5–§5.9.8 的「系统更新显示状态」落到窗上的四枚布尔与一枚角度。§5.9.6 规则说的
    /// 是**更新显示状态**，而在这之前显示状态得先读得到：菜单那六行点的是 invokable，
    /// 按完之后窗上没有任何一格能回答「现在是不是灰的」，量规也就无从判「再按一次恢复」
    /// 与「按了两次等于没按」是同一件事还是两件事。
    ///
    /// 五枚都是**读** core 那一枚字段本体（[`falcon_core::pin::PinItem::grayscale`] 那一族），
    /// 桥侧不长第二份判断（#281）。`rotation` 是 0/90/180/270 而不是「转了几次」：§5.9.5
    /// 那一格要的判据是「以 90 度为基本单位」与「转满四圈回到同一状态」，两者都从角度读得出，
    /// 而次数读不出后者（四圈与零圈在次数上是两枚数、在显示上是同一张图）。
    pub rotation: i32,
    pub flip_h: bool,
    pub flip_v: bool,
    pub grayscale: bool,
    pub inverted: bool,
    /// §5.5.8：这一枚贴图现在配不配被「刷新」。菜单那两行的 `visible` 与
    /// [`PinState::refresh`] 的第一只拒绝臂读的是同一格（判断本体只有
    /// [`PinState::refreshable`] 那一处），所以窗上不会多出一行按下之后只会说「不能刷」的入口
    /// —— 形状同 §5.9.15 的那枚 `has_transparent_pixels`：一枚「这一手有点得动的东西」的格子，
    /// 而不是一行点了没反应的装饰（#494 / #433 那一族）。
    pub refreshable: bool,
}

/// What one §8.5 screen change did to the pins, as three counts and no prose:
/// [`desktop_changed`] turns them into the user's sentence, and a gauge row reads
/// the counts. The three are kept apart because they answer three different
/// questions - did the layout the clamp uses change at all, how many pins had to
/// move, how many were looked at - and a door that folded the first two together
/// could not tell "nothing changed" from "nothing was home".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Refit {
    pub changed: bool,
    pub moved: usize,
    pub total: usize,
}

/// §5.11 独显那一手交回的三件事，各答各的问题（合起来才是一次完整的汇报）：
/// - `changed`：**被带走/被放回了几枚**。0 不发号（#455），但 0 不等于「这一手没做成」——
///   场上只剩被留着的那一枚时，独显真的开了而一枚也不用带走，那一臂由 `active` 说。
/// - `active`：按完之后**在不在独显里**（规则③ 那一问的数，core 的 `solo_active` 答，
///   桥侧不猜）。
/// - `list_moved`：这一手有没有顺手改写名单。菜单那一支会把名单摆成「独显留着的那几枚」，
///   于是别的窗的描边也得重读 —— 那是会话那一侧 `selection_epoch` 那枚号的事，不是
///   `solo_epoch` 的（两件事挤进一格就是 #281）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoloOutcome {
    pub changed: usize,
    pub active: bool,
    pub list_moved: bool,
}

/// §5.12 的失败话：core 那三枚分组错误各说各的，桥侧不合并、也不在界面上重演一遍校验。
/// `name` 是被拒的那枚原名（`NoSuchGroup` 时传空串，因为那一问没有名字可报）。
fn group_say(name: &str, err: &falcon_core::pin::PinError) -> String {
    use falcon_core::pin::PinError as E;
    match err {
        E::EmptyGroupName => "分组名称不能为空".to_string(),
        E::DuplicateGroup(n) => format!("已经有一个叫 {:?} 的分组", n),
        E::NoSuchGroup(id) => format!("没有编号 {id} 的分组{}", {
            if name.is_empty() {
                String::new()
            } else {
                format!("（{}）", name)
            }
        }),
        other => other.to_string(),
    }
}

#[derive(Default)]
pub struct PinState {
    pub set: PinSet,
    pub cfg: PinConfig,
    /// The source picture, in memory. §6.3's snapshot writes these out and reads
    /// them back; until the capture path exists this is the only copy.
    frames: HashMap<PinId, Frame>,
    /// The picture as the pin currently *shows* it - cropped, turned, effected.
    /// This is what §5.9.16's copy and §5.9.17's save hand over, and it is the
    /// "one source bitmap per pin" the plan asks for as the fallback against a
    /// screen-capture path that cannot see our own window (R13).
    rendered: HashMap<PinId, Frame>,
    desk: Desktop,
    /// §5.9.14: the frames a pinned GIF carries, in memory only. Deliberately not
    /// part of §6.3's snapshot - the file the pin came from is on disk and holds
    /// them, and a session that re-pastes the path gets its clip back with them.
    /// Every frame is a whole canvas, so this is the largest thing the live
    /// session holds; [`PinState::close`] is what takes it back.
    clips: HashMap<PinId, GifClip>,
    pushed: HashMap<PinId, u64>,
    revisions: HashMap<PinId, u32>,
    /// §5.9.15 步骤 1: whether each pin's picture has a hole in it, written by
    /// [`Self::sync`] and read by [`Self::view_data`]. A cache rather than a scan
    /// because a reload is not a content change — it happens for a drag, a wheel
    /// tick and every timer tick, and a 4K pin's alpha plane is 33 MB of the answer
    /// staying the same.
    holes: HashMap<PinId, bool>,
    /// §5.9.17 的那批临时文件的**持有者**：本进程交出去过、而剪贴板可能还指着它们的路径。
    ///
    /// 这一枚字段存在的理由就是 PRD 第 1601 行那一句「应避免在退出时删除仍被剪贴板引用的
    /// 文件」：删与不删不能靠"大概没人用了"，得有一个名单。名单只在本进程自己下一次
    /// `EmptyClipboard` 真把旧内容顶掉之后才收缩（见 [`Self::copy_image_with_files`]），
    /// 顶不掉就一枚也不删。
    clip_pending_files: Vec<std::path::PathBuf>,
    /// `%i` 给这一批临时文件的序号：与快速保存那枚 `history_replays` 分着数，因为那是
    /// 用户看得见的一串，这一串是剪贴板看得见的一串。
    clip_seq: u32,
    /// §5.5.11 步骤 3/4 的账：此刻为一张文件保存对话框藏起来的贴图，以及叠着开了几层。
    /// P124 之前只有 [`crate::config::Output::hide_pins_on_dialog`] 那枚键和一个启动横幅，
    /// 这一格把它变成运行期真正生效的那一件事：[`Self::view_data`] 的 `live` 读它，
    /// 于是藏与放只有**一枚**权威，而窗口的可见性仍然是 `PinWindow.qml` 那一行绑定。
    link: Hidden,
    /// §5.15「隐藏/显示全部贴图」与 `toggle_pins` 热键那一路由的**全局**藏匿理由，与 [`Self::link`]
    /// 那枚对话框遮挡**分着数**：#281 要求「一桩事只写一次」，而「用户把全部贴图收起来」和
    /// 「有一张保存对话框压住了几枚」是两桩事，各占一格。合成一格就会漏 —— [`Self::show_all_pins`]
    /// 若与遮挡共用一格，放全局的那一刻会把本该仍为对话框藏起来的那枚错误地放回桌面。
    ///
    /// 默认 `false`（没写过这枚的用户走的是「贴图各自按自身可见」那条老路）。它只进
    /// [`Self::view_data`] 的 `live`，不动 core 的 `PinItem::visible()`：后者记的是**这枚贴图自己**
    /// 被关掉没有（§5.9.18 销毁、§5.11 独显），全局收起是叠在其上的一层理由，不是把它逐个改写。
    all_hidden: bool,
    /// §5.9.18 / §5.10.4「销毁」交回的那一叠槽位：PRD 第 1148 行那句「应显示确认或支持撤销」
    /// 在桥侧落成的就是这一格 —— 没有它，[`falcon_core::pin::PinSet::restore`] 那只手永远
    /// 拿不到东西可还，撤销就只剩一句空话。
    ///
    /// 只存**最近一手**：下一手销毁顶掉上一手的账（[`Self::destroy_pin`] 里那一行 `clear`），
    /// 因为槽位记的是「离开时占的那一格」，两批叠着还就会把第二批插进第一批的位置上。
    trash: Vec<PinSlot>,
    /// 销毁掉的那些枚的**像素**。core 的 `PinSlot` 还得起窗口的位置与效果，还不起一张位图 ——
    /// 而撤销之后那扇窗若是空白的，就不是「支持撤销」而是「撤销成一扇坏窗」。
    /// [`Self::restore_pins`] 把它们搬回 [`Self::frames`]，`sync` 自会把纹理重新推给 Qt。
    trash_frames: HashMap<PinId, Frame>,
    /// 同一封信的 GIF 那一半：§5.9.14 那些帧只活在内存里（不进取证快照），销毁时不搬走，
    /// 撤销回来就是一枚不会动的静图。
    trash_clips: HashMap<PinId, GifClip>,
    /// What went wrong loudly enough that a run has to report it, kept so
    /// `--selftest` can assert on an empty list instead of on stdout.
    pub problems: Vec<String>,
    /// PRD 447's reminder, said by [`Self::remind_before_save`] and by
    /// [`Self::save_image`]'s first rung - always the sentence about a write that has
    /// not happened yet, so it never carries the result of one. The mask's own copy of
    /// this slot is `MaskState::pre_notice`; the two are the same string by
    /// construction because both come out of [`crate::save::reminder`].
    pub pre_notice: String,
}

impl PinState {
    fn new() -> Self {
        let mut problems = Vec::new();
        let desk = desktop_from_platform(&mut problems);
        // §5.5.4–14：整段 `[pin]` 自 P64 起由 `crate::settings::pin_cfg()` 送进运行期 ——
        // P63 之前这里是 `..Default::default()`，`PinConfig::default()` 的九枚键（`default_topmost`
        // / `default_opacity` / `default_zoom` / `default_click_through` / `alpha_bg` /
        // `wheel_resizes` / `cycle_on_click` / `state_save_seconds` / `solo_dim_opacity`）
        // 从来没有拿到用户 config 里写的值。取一次进程启动时的快照，中途 `write_back`
        // 也不会让已经在跑的贴图抖动 —— "默认"就是**新**贴图的默认。
        Self {
            desk,
            problems,
            cfg: crate::settings::pin_cfg(),
            ..Default::default()
        }
    }

    /// A picture becomes a pin at `at`. The id comes from the set, so `next_id`
    /// and a restored session can never collide.
    pub fn add(&mut self, frame: Frame, at: PhysPoint) -> Result<PinId, String> {
        self.place(frame, at, false)
    }

    /// §5.5.8: [`Self::add`] for the one kind of picture a 刷新 can do anything with —
    /// pixels a `capture` leg has just read off the desktop **at `at`**, this bitmap being
    /// exactly that rectangle's contents.
    ///
    /// A leg that puts a picture somewhere other than where it read it keeps calling
    /// [`Self::add`]: recording a rectangle these pixels did not come from would paint the
    /// pin with a picture of something else the next time the user asks it to refresh, and
    /// the claim is the whole reason the field exists (see
    /// [`falcon_core::pin::PinItem::captured_at`]). The four presenters in
    /// [`crate::capture`] are the product's only callers, which is what lets this be the
    /// field's only writer.
    pub fn add_captured(&mut self, frame: Frame, at: PhysPoint) -> Result<PinId, String> {
        self.place(frame, at, true)
    }

    fn place(&mut self, frame: Frame, at: PhysPoint, captured: bool) -> Result<PinId, String> {
        let size = PhysSize::new(frame.width, frame.height);
        let id = self
            .set
            .spawn(falcon_core::pin::ImageRef::Volatile, size, at, &self.cfg)
            .map_err(|e| e.to_string())?;
        if captured {
            match self.set.get_mut(id) {
                Some(p) => p.captured_at = Some(at),
                // `spawn` handed this id out one line ago, so this arm is the set
                // disagreeing with itself. It is recorded rather than trusted: a pin that
                // quietly lost its provenance reads back as「不是从屏幕上截下来的」from then
                // on, and a run that said nothing about how it got that way would be
                // grading itself on a sentence it had already broken.
                None => self.note(format!(
                    "pin {id}: 刚开出来的贴图不在集合里，原截图坐标没记上"
                )),
            }
        }
        self.frames.insert(id, frame);
        self.sync(id);
        Ok(id)
    }

    /// §5.5.8 的那一问，菜单那两行和 [`Self::refresh`] 共读一枚答案：这一枚贴图现在配不配
    /// 被刷新。
    ///
    /// 条件只有「它记不记得自己来自屏幕上哪一块」这一格，与 core 那只拒绝臂是同一格
    /// （[`falcon_core::refresh::plan`] 的第一臂），所以「菜单上有一行、按下来说不能刷」和
    /// 「能刷却没有那一行」都不必留一扇窗来发现。
    ///
    /// GIF 不占一格判断：会动的贴图只从文件里来（[`Self::clips`] 的唯一写入者是
    /// [`Self::attach_clip`]，而它只接在 `add` 之后、从不接在 [`Self::add_captured`] 之后），
    /// 那一路从来不带原截图坐标 —— 两件事不会同时成立，为它写一臂拒绝就是
    /// 一行走不到的分支。
    fn refreshable(&self, p: &PinItem) -> bool {
        p.captured_rect().is_some()
    }

    /// §5.5.8 步骤 2–4 for one pin: 执行「刷新截图」, 按原截图坐标重新读取屏幕内容, and the
    /// two destinations 步骤 4 offers. `overwrite` is 规则 1's choice, taken from the
    /// gesture rather than from a setting — the reason is written on
    /// [`falcon_core::refresh::plan`], and it is not `[output] reuse_last_file`, which is
    /// §5.5.8's *file* half and already has one reader.
    ///
    /// The answer is the sentence the user reads: `Ok` when new pixels went out, `Err` when
    /// nothing moved and 规则 2「区域失效时应提示重新选择」is the whole of what happened. Both
    /// arms reach `Session::status`, which is the slot `HintWindow.qml` reads a pin's
    /// sentences from.
    ///
    /// Deliberately absent: [`capture::remember`]. §5.2.5's 重复上次截图 keeps whatever the
    /// last *capture* asked for, and re-legging that one slot from inside one pin's refresh
    /// would make the next 重复 read this pin's corner instead of the region the user
    /// dragged out — a second requirement answered by a gesture that never mentioned it.
    pub fn refresh(&mut self, id: PinId, overwrite: bool) -> Result<String, String> {
        let want = match self.set.get(id) {
            Some(p) => falcon_core::refresh::plan(p, &self.desk.bounds, overwrite),
            None => {
                return Err(format!(
                    "贴图 {id} 已经不在桌上了：这一手没有落点，屏幕也没有被重读。"
                ))
            }
        };
        match want {
            Refresh::Refused(said) => Err(said),
            Refresh::Overwrite(rect) => {
                let (frame, _, said) = capture::re_read(&rect, &format!("刷新贴图 {id}"))?;
                self.frames.insert(id, frame);
                // The content key cannot see this: same crop, same rotation, same size, so
                // `sync` would answer "nothing moved" and hand Qt nothing while the fresh
                // screen read sat in `frames`. Dropping the cache is what makes the promise
                // 「用新内容更新结果」true rather than a repaint that never arrives.
                self.pushed.remove(&id);
                if !self.sync(id) {
                    return Err(format!(
                        "{said} —— 新内容读到了，但这一张没能送上线：屏幕上还是上一张，复制与保存也还是上一张的内容。"
                    ));
                }
                Ok(said)
            }
            Refresh::NewCopy(rect) => {
                let (frame, read, said) =
                    capture::re_read(&rect, &format!("按贴图 {id} 的原坐标新截"))?;
                let at = PhysPoint::new(read.x, read.y);
                let new = self.add_captured(frame, at)?;
                Ok(format!("{said}，钉成贴图 {new}；贴图 {id} 自己没动。"))
            }
        }
    }

    /// The fields that decide the pixels. Position, zoom and opacity are absent
    /// on purpose: they decide the window. Which GIF frame is on screen is the
    /// exception that proves the rule (§5.9.14) - it decides pixels, not geometry,
    /// and a clock tick left out of here is a frame that never reaches the screen.
    fn content_key(p: &PinItem) -> u64 {
        let region = p.visible_rect();
        let parts: [i64; 11] = [
            region.x as i64,
            region.y as i64,
            region.w as i64,
            region.h as i64,
            p.rotation as i64,
            p.flip_h as i64,
            p.flip_v as i64,
            p.grayscale as i64,
            p.inverted as i64,
            p.alpha_bg_mode as i64,
            p.gif.map(|play| play.frame).unwrap_or(0) as i64,
        ];
        parts.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, v| {
            (h ^ (v as u64)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    /// The picture one pin draws from: the clip's frame that is on screen when
    /// there is a clip, the one picture the file door handed over otherwise.
    /// Cropping, rotation and the alpha background are applied on top of this by
    /// [`falcon_core::pin::PinItem::render_shown`], so a GIF is turned and cropped
    /// frame by frame rather than once.
    fn source_for(&self, id: PinId) -> Option<&Frame> {
        let src = self.frames.get(&id)?;
        match (self.clips.get(&id), self.set.get(id).and_then(|p| p.gif)) {
            (Some(clip), Some(play)) => Some(clip.frame_at(play.frame).unwrap_or(src)),
            _ => Some(src),
        }
    }

    /// §5.9.14: one tick of a window's timer. Returns whether the frame on screen
    /// moved, which is also whether new pixels went out - so a pin that is not a
    /// GIF, a paused clip and a still all answer false and re-encode nothing.
    pub fn tick_gif(&mut self, id: PinId, delta_ms: u32) -> bool {
        let moved = match (self.clips.get(&id), self.set.get_mut(id)) {
            (Some(clip), Some(p)) => p
                .gif
                .as_mut()
                .map(|play| play.tick(delta_ms, &clip.delays_ms))
                .unwrap_or(false),
            _ => false,
        };
        if moved {
            self.sync(id);
        }
        moved
    }

    /// Render, encode and hand Qt the result - but only if the pixels moved.
    /// Returns whether a new texture went out.
    pub fn sync(&mut self, id: PinId) -> bool {
        let key = match self.set.get(id) {
            Some(p) => Self::content_key(p),
            None => {
                self.pushed.remove(&id);
                self.revisions.remove(&id);
                self.holes.remove(&id);
                return false;
            }
        };
        if self.pushed.get(&id) == Some(&key) {
            return false;
        }

        let mut shown: Option<Result<Frame, String>> = None;
        let mut holed = false;
        if let (Some(p), Some(src)) = (self.set.get(id), self.source_for(id)) {
            // §5.9.15's question — is there a hole at all — is asked of the picture
            // the pin actually shows, and once per content change rather than once
            // per reload. Asked *before* the board goes down because
            // [`falcon_core::pin::PinItem::paint_alpha_bg`] fills the holes in a
            // checkerboard mode, and a pin whose menu vanished the moment the user
            // chose 深色棋盘格 could never choose 透明 again.
            shown = Some(
                p.render_shown(src)
                    .map(|f| {
                        holed = f.has_transparent_pixels();
                        p.paint_alpha_bg(f)
                    })
                    .map_err(|e| e.to_string()),
            );
        }
        let shown = match shown {
            Some(Ok(f)) => f,
            Some(Err(e)) => {
                self.note(format!("pin {id}: render failed: {e}"));
                return false;
            }
            None => {
                self.note(format!("pin {id} has no pixels to draw from"));
                return false;
            }
        };
        let opt = EncodeOptions {
            format: Format::Png,
            ..Default::default()
        };
        let png = match encode::encode(&shown, &opt) {
            Ok(b) => b,
            Err(e) => {
                self.note(format!("pin {id}: encode failed: {e}"));
                return false;
            }
        };

        shim::store_frame(id, &png);
        self.rendered.insert(id, shown);
        self.holes.insert(id, holed);
        self.pushed.insert(id, key);
        *self.revisions.entry(id).or_insert(0) += 1;
        true
    }

    /// Apply a view or content change to one pin, then push whatever it produced.
    /// One gesture applied to one pin. `None` when the pin is not on screen,
    /// which is a closed window's leftover click rather than an error.
    pub fn edit<R>(
        &mut self,
        id: PinId,
        f: impl FnOnce(&mut PinItem, &PinConfig, &Desktop) -> R,
    ) -> Option<R> {
        let cfg = self.cfg.clone();
        let desk = self.desk.clone();
        let done = self.set.get_mut(id).map(|p| f(p, &cfg, &desk));
        self.sync(id);
        done
    }

    /// §5.9.15 步骤 2/3: the menu's row `index` becomes the pin's mode, and the
    /// new picture goes out in the same gesture. This is the first reader of
    /// [`PinItem::set_alpha_bg_mode`] that is not a test — the setter and the
    /// painter have agreed since §5.9.15 was coded, and nothing ever asked them to.
    ///
    /// An out-of-range row is noted and changes nothing: `AlphaBg::from_index` has
    /// no "last one" fallback, so a menu that grew a fifth row cannot silently
    /// repaint every pin as the fourth mode.
    pub fn set_alpha_bg(&mut self, id: PinId, index: i32) -> bool {
        let Some(mode) = AlphaBg::from_index(index.max(0) as u32) else {
            self.note(format!("pin {id}: no background mode #{index}"));
            return false;
        };
        self.edit(id, |p, _, _| p.set_alpha_bg_mode(mode)).is_some()
    }

    /// §5.9.15's other half: whether the point a press landed on takes the click.
    /// [`PinItem::hit`] answered this all along and had no reader outside its own
    /// tests, so 透明区域不可点击 was a rule the encoder honoured and the window did
    /// not.
    ///
    /// `at` is a point in the physical desktop, the space `window_to_source` starts
    /// from; the caller that has a cursor *inside* a window adds that window's
    /// position first ([`crate::pin_view`]'s `hitAt` does). A pin that is gone
    /// answers "no click", like every other question here.
    pub fn hit_at(&self, id: PinId, at: PhysPoint) -> bool {
        match (self.set.get(id), self.source_for(id)) {
            (Some(p), Some(src)) => {
                p.hit(at, |s| src.alpha_at(s.x.max(0) as u32, s.y.max(0) as u32))
            }
            _ => false,
        }
    }

    pub fn view_data(&self, id: PinId) -> PinViewData {
        let Some(p) = self.set.get(id) else {
            return PinViewData {
                live: false,
                dpr: 1.0,
                ..Default::default()
            };
        };
        // The centre is the screen the pin is *on*: a corner lags a whole window
        // behind during a drag across a boundary, and §5.9.1 lets the window hang
        // into dead space, which `dpr_at` answers with the primary rather than nothing.
        //
        // `window_rect` is the same `pos_phys`+`size_phys` pair [`Self::pin_rects`] hands
        // to 谁遮挡, so the rectangle the ratio is taken from and the rectangle the hide
        // decision is taken from are one reading rather than two spellings (#281 那一族).
        let window = p.window_rect();
        let play = p.gif;
        PinViewData {
            live: !self.all_hidden && !self.link.hidden().contains(&id) && p.visible(),
            pos_x: p.pos_phys.x,
            pos_y: p.pos_phys.y,
            size_w: p.size_phys.w as i32,
            size_h: p.size_phys.h as i32,
            dpr: self.desk.dpr_at(window.center()).ratio(),
            zoom: p.zoom as i32,
            opacity: p.opacity as i32,
            smooth: p.smooth_zoom,
            topmost: p.topmost,
            click_through: p.click_through,
            revision: self.revisions.get(&id).copied().unwrap_or(0),
            gif_frames: play.map(|g| g.frames).unwrap_or(0) as i32,
            gif_frame: play.map(|g| g.frame).unwrap_or(0) as i32,
            gif_playing: play.map(|g| g.playing).unwrap_or(false),
            gif_speed: play.map(|g| g.speed).unwrap_or(0) as i32,
            alpha_bg: p.alpha_bg_mode.index() as i32,
            has_transparent_pixels: self.holes.get(&id).copied().unwrap_or(false),
            // 一份名单，两个读者：窗侧那一格与 core 那枚 `BTreeSet` 必须同集合，
            // 所以这里不查副本、直接问名单本体（`pin_check.rs` 里那枚 `drawn()` 判的就是它）。
            selected: self.set.is_selected(id),
            solo_hidden: p.solo_excluded,
            rotation: p.rotation,
            flip_h: p.flip_h,
            flip_v: p.flip_v,
            grayscale: p.grayscale,
            inverted: p.inverted,
            refreshable: self.refreshable(p),
        }
    }

    /// §5.10.1 步骤 1/2 的那一手：单击只留这一枚（`add=false`），Ctrl+单击加、再按同一枚是减
    /// （`add=true`）。交回的是**按完之后名单里有几枚**，不是"改没改"—— 桥那一侧的
    /// `selection_count` 与退出多选那句「取消了 N 张贴图的选中状态」读的都是这一个数，
    /// 两处各数一遍就会分成两套（#281 那一族）。
    ///
    /// 贴图不存在时交 `Err` 且**名单一枚也不动**：菜单陈旧、窗已关掉，都是调用方的 bug，
    /// 把它扮成"悄悄清场"或"悄悄失败"会让用户以为自己的手势生效了。
    pub fn select_pin(&mut self, id: PinId, add: bool) -> Result<usize, String> {
        self.set
            .select(id, add)
            .map(|_| self.set.selection_count())
            .map_err(|e| e.to_string())
    }

    /// §5.10.1 步骤 4：退出多选。交回的是**这一手清掉了多少枚**，不是清完再数（那样永远是
    /// 0，拿它当答案就是把一句实话换成一枚恒真的读数 #455）。`0` 与「没有这一手」在这里是
    /// 同一件事，所以交数不交 `Option`（#453）。
    pub fn clear_selection(&mut self) -> usize {
        let took = self.set.selection_count();
        self.set.clear_selection();
        took
    }

    /// 此刻名单的条数。桥的 qproperty 与退出那句共读这一格。
    pub fn selection_count(&self) -> usize {
        self.set.selection_count()
    }

    /// 名单本身，升序。§5.10.2/3/4 那三批批量手势要用的是这一份，不是数量。
    pub fn selected_ids(&self) -> Vec<PinId> {
        self.set.selected_ids()
    }

    /// §5.10.2 — one drag, and it is the **list** that decides how many pins move,
    /// not the window that happened to get the pointer. The pin under the cursor
    /// always moves; when it is one of several selected, every selected pin takes
    /// the same displacement, each clamped on its own by
    /// [`falcon_core::pin::PinSet::translate_selection`] — a pin that reaches a
    /// screen edge stops there without pinning the rest of the batch to it.
    ///
    /// A pin that is not in the list is the §5.9.1 case and stays that case: one
    /// [`falcon_core::pin::PinItem::move_by`], one sync. Nothing here requires a
    /// selection to have happened first, which is what keeps "drag a pin you never
    /// clicked" from silently becoming "drag every pin on the desktop" — the
    /// inverse bug is the one the window would have to write, so the window does
    /// not get to ask this question (its charter says so; the gauge leg in
    /// `PinWindow.qml` calls this hand and reads back only its own window).
    ///
    /// 交回的是**搬动了几枚**，不是"成没成"：`0` 只有一枚贴图能给出——那一枚不在集里，
    /// 是一扇已关掉的窗剩下的鼠标事件。把它扮成一次悄悄的成功，用户就会以为自己的拖动
    /// 搬动了一枚并不存在的贴图（#455）。
    pub fn drag_pin(&mut self, id: PinId, dx: i32, dy: i32) -> usize {
        if self.set.get(id).is_none() {
            return 0;
        }
        if self.batch_hand(id) {
            let ids = self.set.selected_ids();
            let moved = self.set.translate_selection(dx, dy, &self.desk);
            // 每一枚都要被重新问一遍。`content_key` 不含位置，所以这一圈不会重画像素：
            // 它问的是"这一枚的窗此刻该画在哪儿"，而那正是批量要送到屏幕上的东西。
            for one in ids {
                self.sync(one);
            }
            return moved;
        }
        self.set.get_mut(id).map(|p| p.move_by(dx, dy, &self.desk));
        self.sync(id);
        1
    }

    /// 「这一手算几枚」在这一格判**一次**（#281：同一件事两处各数一遍）：被点中的那一枚在
    /// 名单里、且名单里还有第二枚，就是批量；否则是那一枚自己的那一手。§5.10.2 的拖动与
    /// §5.10.3 的两手透明度读的是同一个判据，所以它不许在桥侧、也不许在窗侧再长第二份。
    fn batch_hand(&self, id: PinId) -> bool {
        self.set.is_selected(id) && self.set.selection_count() > 1
    }

    /// §5.10.3 — one opacity adjustment, and as with §5.10.2 it is the **list** that
    /// decides how many pins change, not the window that happened to hold the wheel or
    /// the menu. The decider is [`Self::batch_hand`], written once.
    ///
    /// A pin that is not in a multi-selection is the §5.9.4 case and stays that case:
    /// one [`falcon_core::pin::PinItem::set_opacity`], one sync. `opacity` is not in
    /// `content_key` (see [`Self::view_data`]), so this hand never re-encodes a
    /// picture — it only asks every window in the list again where it stands.
    ///
    /// 交回的是**改了几枚**：`0` 同上——那一枚不在集里，是一扇已关掉的窗剩下的那一次滚轮。
    pub fn set_pin_opacity(&mut self, id: PinId, pct: u32) -> usize {
        if self.set.get(id).is_none() {
            return 0;
        }
        if self.batch_hand(id) {
            let ids = self.set.selected_ids();
            let n = self.set.set_selection_opacity(pct);
            for one in ids {
                self.sync(one);
            }
            return n;
        }
        if let Some(p) = self.set.get_mut(id) {
            p.set_opacity(pct);
        }
        self.sync(id);
        1
    }

    /// §5.10.3 的另一半：阶梯。「更透明 / 更实」调的是每一枚**自己**当前那一级，所以 30% 与
    /// 80% 两枚一起按「更透明」得到的是 20% 与 70%；把它们压成同一个数就不是「同步调整」而是
    /// 「重新设定」了——与 §5.10.2 那一句「按相同位移移动」同一判据（[`falcon_core::pin::PinSet::step_selection_opacity`]）。
    pub fn step_pin_opacity(&mut self, id: PinId, up: bool) -> usize {
        if self.set.get(id).is_none() {
            return 0;
        }
        if self.batch_hand(id) {
            let ids = self.set.selected_ids();
            let n = self.set.step_selection_opacity(up);
            for one in ids {
                self.sync(one);
            }
            return n;
        }
        self.set.get_mut(id).map(|p| p.opacity_step(up));
        self.sync(id);
        1
    }

    /// §5.10.4「重置」：把**怎么看**这一枚还给默认（缩放、旋转、翻转、效果、透明度、置顶、
    /// 穿透），而裁剪与位置不动 —— 那一句是 [`falcon_core::pin::PinItem::reset_view`] 自己
    /// 答的（"a reset is not a move"），本档不重述它的清单，只决定改几枚（#281）。
    ///
    /// 单枚那一臂走的是 [`falcon_core::pin::PinItem::reset_view`]，与 P134 之前 `PinView`
    /// 自己那一路同一只手；差别在于这一格不需要窗来问「本窗该不该带上别的」——窗的章程写着
    /// 它不决定这件事。
    ///
    /// 交回的是**重置了几枚**：`0` 是那一枚不在集里（一扇已关的窗剩下的菜单点击）。
    pub fn reset_pin(&mut self, id: PinId) -> usize {
        if self.set.get(id).is_none() {
            return 0;
        }
        if self.batch_hand(id) {
            let ids = self.set.selected_ids();
            let n = self.set.reset_selection(&self.cfg);
            for one in ids {
                self.sync(one);
            }
            return n;
        }
        if let Some(p) = self.set.get_mut(id) {
            p.reset_view(&self.cfg);
        }
        self.sync(id);
        1
    }

    /// 「这一手带走的是**哪几枚**」在这一格答**一次**：批量就是整张名单，否则就是被点中的那一枚；
    /// 那一枚已经不在集里就是**一枚也没有**（空表 —— [`Self::close_pin`] 与
    /// [`Self::destroy_pin`] 都靠这一格交回 0，而不是交回 1 冒充一次成功，#455）。
    ///
    /// 为什么它不并到 [`Self::batch_hand`] 里：那一只手答的是「算不算批量」，搬动与调透明度
    /// 那两手要它是因为**批量与单枚走的是 core 里两只不同的手**（`translate_selection` 对
    /// `move_by`）；退役那两只只在意名单长短，所以它们要的是这张表。两份都在同一格，窗侧一份也没有。
    fn hand_targets(&self, id: PinId) -> Vec<PinId> {
        if self.set.get(id).is_none() {
            return Vec::new();
        }
        if self.batch_hand(id) {
            self.set.selected_ids()
        } else {
            vec![id]
        }
    }

    /// §5.10.4「关闭」的那一只手：与上面三手读同一枚 [`Self::batch_hand`]（经由
    /// [`Self::hand_targets`]）。
    ///
    /// 退役的账（纹理、位图、GIF 帧）走 [`Self::close`]，那一只手在这里只是**决定名单**之后
    /// 的那一次调用 —— 不在这里重复清理，也不在窗那一侧数「名单里有几枚」（#281）。
    ///
    /// 交回的是**关掉了几枚**；`0` 同上。
    pub fn close_pin(&mut self, id: PinId) -> usize {
        let ids = self.hand_targets(id);
        if ids.is_empty() {
            return 0;
        }
        self.close(&ids)
    }

    /// §5.10.4「影响数量」：**这一手**会带走几枚，只在这一格答（PRD 第 1149 行要的就是这个
    /// 数，而它在桥侧此前一位读者也没有）。
    ///
    /// 答的是「按下去会带走几枚」而不是「刚才带走了几枚」：菜单在用户点下去**之前**要把这句话
    /// 印在字面上，所以这一格与 [`Self::destroy_pin`] 走的是同一张 [`Self::hand_targets`] ——
    /// 两处各数一遍就是 #281 那一族，而那一处的错用户看得见（印着的数与真被带走的那些不一致）。
    ///
    /// 批量那一臂读的是 core 那一格 [`falcon_core::pin::PinSet::destroy_count`]：「销毁」这个词
    /// 在状态机里该带走几枚，那里已经答过一次了，这一格不再自己数名单长度。
    pub fn destroy_answer(&self, id: PinId) -> usize {
        if self.batch_hand(id) {
            self.set.destroy_count()
        } else {
            self.hand_targets(id).len()
        }
    }

    /// §5.9.18 / §5.10.4「销毁」：从集里彻底拿走，而唯一的回头路就是这一格收进
    /// [`Self::trash`] 的那一叠槽位。带走哪几枚由 [`Self::hand_targets`] 答（同一枚判据，
    /// 第四处不写第二份）。
    ///
    /// 与 [`Self::close_pin`] 的两件不同，逐件都有理由：
    /// 1. 位图与 GIF 帧**不**丢掉，而是搬进 `trash_*` —— 撤销回来的那扇窗必须还有像素；
    /// 2. 名单在 core 那一侧由 `after_removal` 顺手摘掉被带走的那些枚，所以本手**不**再
    ///    `sync`：已经没有窗了，`shim::drop_frame` 才是这一手对 Qt 说的话。
    ///
    /// 交回的是**带走了几枚**；`0` 同上（一扇已关的窗剩下的那一次点击）。
    pub fn destroy_pin(&mut self, id: PinId) -> usize {
        let ids = self.hand_targets(id);
        if ids.is_empty() {
            return 0;
        }
        let slots = self.set.destroy(&ids);
        // 上一手的账在这一手之前必须结清：撤销只认**最近**那一手（见 `trash` 那一格的信）。
        self.trash.clear();
        self.trash_frames.clear();
        self.trash_clips.clear();
        for slot in &slots {
            let one = slot.item.id;
            if let Some(f) = self.frames.remove(&one) {
                self.trash_frames.insert(one, f);
            }
            if let Some(c) = self.clips.remove(&one) {
                self.trash_clips.insert(one, c);
            }
            self.retire(one);
        }
        let n = slots.len();
        self.trash = slots;
        n
    }

    /// §5.9.18「若销毁不可恢复，应显示确认或支持撤销」的另一臂：把最近那一手带走的东西
    /// 放回**它们各自离开的那一格**（core 的 `PinSlot.index` 记的就是那个位置，所以一堆
    /// 撤销回来还是原来那堆，而不是摞在最上面）。
    ///
    /// 先搬像素再 `sync`：`pushed` 在那一手已经被清掉了，`sync` 于是必然重推一次纹理 ——
    /// 这正是撤销回来那扇窗该有的代价，而不是一扇空白窗。
    ///
    /// 交回的是**还回了几枚**；账是空的就交回 0（那一手压根没有撤销可做，`0` 不发号）。
    pub fn restore_pins(&mut self) -> usize {
        if self.trash.is_empty() {
            return 0;
        }
        let slots = std::mem::take(&mut self.trash);
        let ids: Vec<PinId> = slots.iter().map(|s| s.item.id).collect();
        for one in &ids {
            if let Some(f) = self.trash_frames.remove(one) {
                self.frames.insert(*one, f);
            }
            if let Some(c) = self.trash_clips.remove(one) {
                self.clips.insert(*one, c);
            }
        }
        let n = self.set.restore(slots);
        for one in &ids {
            self.sync(*one);
        }
        n
    }

    /// 此刻还欠着几枚撤销（[`Self::restore_pins`] 的可见性由这一格答，不在窗那一侧数）。
    pub fn trashed(&self) -> usize {
        self.trash.len()
    }

    /// §5.11 独显「留哪几枚」的那张表，两条入口在这一格并成一条：
    /// - `Some(id)` 是窗上菜单那一支 —— 留的就是 [`Self::hand_targets`] 已经答过的那一张
    ///   （名单里的一枚＝那一批，名单外的一枚＝只这一枚；§5.10.4 那枚判据的第四处读者，
    ///   这里不写第二份）；
    /// - `None` 是 `ctrl+shift+s` 那一支 —— 它没有「被点中的那一枚」，PRD 步骤 1 那句
    ///   「用户选中一张或多张贴图」就是它的全部输入，所以留的是此刻的名单本身。
    ///
    /// 它单列一格，是因为 [`Self::solo_pin`] 的**扇出**必须走全体（被独显带走的那些枚恰恰
    /// 不在名单里）；把名单那把尺的名字留在同一只手里，下一个读代码的人就分不清哪一处才是
    /// 「谁要重画」的答案。
    fn solo_kept_table(&self, id: Option<PinId>) -> Vec<PinId> {
        match id {
            Some(one) => self.hand_targets(one),
            None => self.set.selected_ids(),
        }
    }

    /// §5.11 的那一只**手**，菜单与热键两条入口都到这一格。走哪一只由 core 的 `solo_active`
    /// 答，交回的数由 core 那两只各交 `usize` 的手给（`enter_solo` / `exit_solo`）—— 桥侧
    /// 既不数 `items`，也不接 core 那枚 `toggle_solo` 的 `bool`：`false` 在那里同时容纳
    /// 「刚退出」与「没选中被拒绝」，而这一族递得下去的正是那个数目（#455）。
    ///
    /// 扇出走**全体**：§5.11 步骤 3 带走的是名单**之外**的那些枚，所以每一扇窗都得重读一次
    /// `live`（[`Self::view_data`] 那一行）与 `opacity`。一只按名单走的扇出恰好放过该藏的
    /// 那些枚 —— 读起来像绿了，而那是最贵的一种错。
    pub fn solo_pin(&mut self, id: Option<PinId>) -> SoloOutcome {
        if self.set.solo_active() {
            let back = self.set.exit_solo();
            for one in self.ids() {
                self.sync(one);
            }
            return SoloOutcome {
                changed: back,
                ..Default::default()
            };
        }
        let kept = self.solo_kept_table(id);
        if kept.is_empty() {
            return SoloOutcome::default();
        }
        let same = self.set.selection_count() == kept.len()
            && kept.iter().all(|one| self.set.is_selected(*one));
        self.set.set_selection(&kept);
        let hidden = self.set.enter_solo(&self.cfg);
        for one in self.ids() {
            self.sync(one);
        }
        SoloOutcome {
            changed: hidden,
            active: self.set.solo_active(),
            list_moved: !same,
        }
    }

    /// §5.11 规则③ 那一问在桥侧的形状：**独显留着几枚**。读的是 core 那张 `Solo::kept`
    /// 表的长度，不在这里另数一遍（#281）。没开独显就是 0 —— 而它不可能是「开着却留 0 枚」，
    /// 因为 core 的 `enter_solo` 对空名单直接交回 0 且不记账。会话那一侧的 `solo_kept`
    /// 绑的就是这一格，于是窗上的字面、屏幕提示与门读的是同一个数。
    pub fn solo_kept(&self) -> usize {
        self.set.solo_keeps().len()
    }

    /// §5.11 菜单字面与量规共用的那一枚答案：**这一手**会带走几枚（在用户点下去之前问）。
    /// 它与 [`Self::solo_pin`] 走的是同一张 [`Self::solo_kept_table`]，所以印着的数与真被
    /// 带走的那些枚不可能各走各的 —— 与 [`Self::destroy_answer`] 同一封信。
    ///
    /// 独显已经开着时交回 0：此时那一句字面是「退出独显」，它要说的数是**放回**几枚，而放回
    /// 的那几张是 core 进独显那一刻记下的账（`Solo::saved`，桥侧一层也不许留副本）。退出那一支
    /// 的字面因此不带数目 —— 硬编一个就是拿猜测当读数。
    pub fn solo_answer(&self, id: Option<PinId>) -> usize {
        if self.set.solo_active() {
            return 0;
        }
        let kept = self.solo_kept_table(id);
        if kept.is_empty() {
            return 0;
        }
        self.set.ids().len().saturating_sub(kept.len())
    }

    /// §5.11「其余那几枚」到底是**压暗**还是**隐藏**：读的是配置键 `pin.solo_dim_opacity`
    /// 本体（默认 35 ⇒ 压暗），这一格是它在桥侧的第一位读者。菜单字面与量规都问这里，而不是
    /// 各自写死「隐藏」—— PRD 步骤 3 那句「系统隐藏未选中的贴图」在 core 里早就长成
    /// 「按配置压暗或隐藏」两臂，字面跟着行为走才是实话。
    pub fn solo_dims(&self) -> bool {
        self.cfg.solo_dim_opacity > 0
    }

    /// §5.12 分组的整张快照，按 core `PinSet::groups` 里那一份**顺序**返回：
    /// `(id, 名, 整组此刻显不显, 组里几枚)`。顺序就是 PRD §5.12.3 那句「排序变化应同步反映
    /// 到快捷切换菜单」要的那个顺序 —— 桥侧不重排（#281：名单只有一份，谁排由 core 那一格答）。
    ///
    /// 计数与显隐都读的是 [`falcon_core::pin::PinSet`] 本体（`pins_in_group` / `group_shown`），
    /// 桥侧一格也不自己数 `items`：那是 core 已经答过的事。
    pub fn group_rows(&self) -> Vec<(GroupId, String, bool, usize)> {
        self.set
            .groups()
            .iter()
            .map(|g| {
                (
                    g.id,
                    g.name.clone(),
                    self.set.group_shown(g.id),
                    self.set.pins_in_group(g.id).len(),
                )
            })
            .collect()
    }

    /// §5.12.1 步骤 3/4：新建一枚空分组。名字空/重名由 core 那一格判（`group_name` 与
    /// `name_taken`），桥侧只把它的 `PinError` 翻成一句能上屏幕的话。空名与重名是两种失败，
    /// 交回的话也分两样 —— 让用户猜是哪一样撞车了就是拿猜测当读数。
    pub fn create_group(&mut self, name: &str) -> Result<GroupId, String> {
        self.set.create_group(name).map_err(|e| group_say(name, &e))
    }

    /// §5.12.3「重命名」：同名校验走 core 那一格，本手**不动任何贴图**，所以不 `sync` ——
    /// 改的是分组的名字，不是任何一扇窗的可见性或像素。
    pub fn rename_group(&mut self, id: GroupId, name: &str) -> Result<(), String> {
        self.set
            .rename_group(id, name)
            .map_err(|e| group_say(name, &e))
    }

    /// §5.12.3「删除」+ 那句「删除非空分组时应询问贴图如何处理」的那一问。`destroy` 是把
    /// 「随组销毁」还是「退回未分组」交回 core 的 [`GroupFate`]：
    /// - `false` → [`GroupFate::Ungroup`]：贴图留在场上、`group_id` 归 `None`，成员没变 ⇒ 不 `sync`；
    /// - `true` → [`GroupFate::Destroy`]：贴图随组走，**成员**变了 ⇒ 交给调用方的 `refresh()`
    ///   把走掉的那些窗收掉（与 [`Self::destroy_pin`] 同一封信：成员变借 `count`，不另发号）。
    ///
    /// 交回的是**被处理了几枚贴图**，正是一句确认话要印的数。
    pub fn delete_group(&mut self, id: GroupId, destroy: bool) -> Result<usize, String> {
        let fate = if destroy {
            GroupFate::Destroy
        } else {
            GroupFate::Ungroup
        };
        let n = self
            .set
            .delete_group(id, fate)
            .map_err(|e| group_say("", &e))?;
        if destroy {
            for one in self.ids() {
                self.sync(one);
            }
        }
        Ok(n)
    }

    /// §5.12.2 步骤 2/3/4：把**此刻的名单**（选中的那些枚）移进 `group`，`None` 是「移出分组」。
    /// 读的是 core `assign_selection_to_group`，它内部再走 `selected_ids` —— 名单只有一份，
    /// 桥侧不递第二张。返回**被改归属的枚数**（`None` 时那批是「原来在这组里的」）。
    ///
    /// 这一步只改 `group_id`，不改任何一扇窗的可见性或像素，所以**不 `sync`**：窗不重画，
    /// 变的只是分组快照那一串字，由调用方重写 [`crate::session`] 的 `groups` 那一格即可。
    pub fn assign_selection_to_group(&mut self, group: Option<GroupId>) -> Result<usize, String> {
        self.set
            .assign_selection_to_group(group)
            .map_err(|e| group_say("", &e))
    }

    /// §5.12.2 的那一只手**点名**版本：同一枚 core `assign_group`，只是名单由调用方递进来而不是
    /// 从「此刻选中的那些枚」读。窗上那一只手要的从来是后者（真人用勾选挑枚），而**量具**要的是
    /// 前者 —— 一条腿要在没有任何一扇窗被点过的情况下把三枚贴图分好家，读选中名单就只能先制造
    /// 一次点击，那是拿量具的输入冒充用户的一次操作。
    ///
    /// 与 [`Self::assign_selection_to_group`] 同样不动像素 ⇒ 不 `sync`，变的只是分组快照。
    pub fn assign_group(&mut self, ids: &[PinId], group: Option<GroupId>) -> Result<usize, String> {
        self.set
            .assign_group(ids, group)
            .map_err(|e| group_say("", &e))
    }

    /// §5.12.3「拖动排序」：`delta` 是 ±1（上移/下移），core 自己饱和夹边。同样不动像素，
    /// 只动那一串字的**顺序** ⇒ 不 `sync`。
    pub fn move_group(&mut self, id: GroupId, delta: i32) -> Result<(), String> {
        self.set
            .move_group(id, delta)
            .map_err(|e| group_say("", &e))
    }

    /// §5.12.4 步骤 2/3「显示 / 隐藏 / 切换」整组：这一手**真的**改组内每一枚的 `group_hidden`
    /// ⇒ 走 [`Self::view_data`] 的 `live`。扇出走**全体**（与 §5.11 独显同一封信：被藏起来的那
    /// 些枚恰恰是这一手的目标，只刷被点中的那扇会放过该改的那些）。交回**切换之后显不显**，
    /// 让桥侧那句号和界面那一眼都读同一个答案。
    pub fn toggle_group_shown(&mut self, id: GroupId) -> Result<bool, String> {
        let shown = self
            .set
            .toggle_group_shown(id)
            .map_err(|e| group_say("", &e))?;
        for one in self.ids() {
            self.sync(one);
        }
        Ok(shown)
    }

    /// §5.15「隐藏全部贴图」/ `toggle_pins` 热键的藏那一臂：置起 [`Self::all_hidden`] 这枚全局
    /// 理由，然后**扇出全体** —— 被藏起来的那些枚恰恰就是这一手的目标，只刷被点中的那扇会放过
    /// 它们。与 [`Self::toggle_group_shown`] 同一封信。交回切换之后的全局态（`true` = 已收起）。
    ///
    /// 这里**不碰 core 的 `PinItem::visible()`**：全局收起是叠在每枚贴图自身可见性之上的一层理由，
    /// 逐个改写自身可见性会让 [`Self::show_all_pins`] 无从分辨「本来就被用户关掉的那枚」与「只是
    /// 被全局收起的那枚」，放回来的时候就会把前者也错误地打开。
    pub fn hide_all_pins(&mut self) -> bool {
        self.set_all_hidden(true)
    }

    /// §5.15「显示全部贴图」/ `toggle_pins` 热键的放那一臂：落下 [`Self::all_hidden`]。交回
    /// 切换之后的全局态（`false` = 已放开）。为对话框遮挡或整组隐藏单独藏起来的那些枚**不会**
    /// 因这一手回到桌面 —— 那是另外两桩事，各有各的一格理由（#281）。
    pub fn show_all_pins(&mut self) -> bool {
        self.set_all_hidden(false)
    }

    /// §5.15 步骤里那句「切换」，也是 `toggle_pins` 热键走的那一臂：读当前全局态、取反、扇出。
    /// 交回**取反之后**的全局态，让 §5.11 独显那种「界面那一眼与桥侧那句号读同一个数」在这里同样成立。
    pub fn toggle_all_pins(&mut self) -> bool {
        let next = !self.all_hidden;
        self.set_all_hidden(next)
    }

    /// 三手共用的那一格：写下全局态、把每一枚贴图的视图重算推给 Qt。返回写下去的那个值。
    fn set_all_hidden(&mut self, hidden: bool) -> bool {
        self.all_hidden = hidden;
        for one in self.ids() {
            self.sync(one);
        }
        hidden
    }

    /// 此刻全局是不是「已收起」。§[`crate::session`] 那枚 `all_hidden` qproperty 读这里，
    /// 而它绑**值**不绑调用，所以热键按下之后界面那一眼会自己跟上（P37 那条口径）。
    pub fn all_pins_hidden(&self) -> bool {
        self.all_hidden
    }

    /// §5.5.11 步骤 3 的输入形状：此刻还在集里的每一枚贴图，它的窗口在物理桌面上占的那一块。
    ///
    /// 集里的每一枚都在这里，**包括已经被藏起来的那些**：下一层对话框可能落在另一个位置，
    /// 压住一批这一层没压住的贴图，而 [`Hidden::opened`] 自己会去掉重复的那些枚。反过来，
    /// 已经关掉的那一枚贴图不在这张表里 —— 它没有窗口可藏，恢复它等于替用户开一扇新窗。
    pub fn pin_rects(&self) -> Vec<(PinId, PhysRect)> {
        self.set
            .items()
            .iter()
            .map(|p| (p.id, p.window_rect()))
            .collect()
    }

    /// §5.5.11 步骤 3：一扇文件保存对话框出现了。交回**此刻新藏**的那些枚，调用方只负责
    /// 让窗口重新读一次 [`Self::view_data`]（`main.qml` 里那一圈 `view.reload()`）。
    ///
    /// `enabled` 是 [`crate::config::Output::hide_pins_on_dialog`]，由门那一侧读一次递进来
    /// （[`crate::settings::output_cfg`]）；关着的时候连"谁遮挡"都不问 —— 步骤 1 那一格是
    /// 用户决定，不是这一层能替用户否决的事，也不是一个可以省掉的成本。
    ///
    /// `dialog` 是那扇对话框此刻占的那一块桌面。`None` 是**普通**的一臂，不是失败：Qt 把
    /// `FileDialog` 标成可见要比 Win32 真造出那扇 `#32770` 早约 400 ms（`PinWindow.qml`
    /// 头顶记着那一枚测量），所以事件到达时窗还不存在、也就没有矩形可读。那一臂藏的是
    /// **全部**还在集里的贴图 —— 这是"可能遮挡"最保守的读法：按钮点不到的那种失败用户
    /// 看不见原因，多藏几枚他们看得见、并且对话框一关就都回来。
    pub fn dialog_opened(&mut self, enabled: bool, dialog: Option<PhysRect>) -> Vec<PinId> {
        // 层数归零 ⇒ 上一批的账已经交回过了，这一次是一张全新的账：开关按**此刻**读到的
        // 那份重新生效（用户可以在这两趟之间去改配置文件，`[output cfg]` 那一行说的就是它）。
        if self.link.dialogs() == 0 {
            self.link = Hidden::new(enabled);
        }
        if !enabled {
            return Vec::new();
        }
        let candidates = match dialog {
            Some(rect) => dialog_link::covering(&self.pin_rects(), rect),
            None => self.ids(),
        };
        self.link.opened(&candidates)
    }

    /// §5.5.11 步骤 4：一扇对话框关闭了。交回**该重新显示**的那些枚 —— 还留着别层时交空，
    /// 最后一层关掉时才把整批放回（[`Hidden::closed`] 那一段是理由）。
    pub fn dialog_closed(&mut self) -> Vec<PinId> {
        self.link.closed()
    }

    /// §5.5.11 步骤 3 的那一判，**只读不写**：把这一步此刻的样子印成一行，不改任何一笔账。
    ///
    /// [`Self::dialog_opened`] 交回的是"此刻新藏的那几枚"，而这一枚交回的是"如果对话框就站
    /// 在这儿，谁会盖住它"——两者在 `None` 那一臂上是同一个答案（都不藏），在 `Some(rect)`
    /// 上不是（挑的是几何，而不是整批）。分开设一枚判据的理由是运行期那一格：门要引的读数必须
    /// 能在**不弄走任何贴图**的前提下取到，否则这一格自己就成了它自己判据的前置。
    ///
    /// `dialog` 由调用方现读（[`crate::session`] 的 `pinDialogRect`），本层不碰 Win32 —— 与
    /// [`Self::dialog_opened`] 同一分层理由。行里四段各自有名：
    /// `rect=` 是那扇窗占的桌面块（读不到 ⇒ `-`，三种缺席同读法，见 `cpp/pin_shim.h`），
    /// `covering=` 是几何挑出的那几枚（空 ⇒ `-`），`pins=` 是集合里此刻几枚，后三段是
    /// [`Self::dialog_line`] 本体。
    pub fn dialog_probe(&self, dialog: Option<PhysRect>) -> String {
        let covering: Vec<String> = match dialog {
            Some(rect) => dialog_link::covering(&self.pin_rects(), rect)
                .iter()
                .map(|id| id.to_string())
                .collect(),
            None => Vec::new(),
        };
        format!(
            "rect={} covering={} pins={} {}",
            match dialog {
                Some(rect) => format!("{},{},{},{}", rect.x, rect.y, rect.w, rect.h),
                None => "-".to_string(),
            },
            if covering.is_empty() {
                "-".to_string()
            } else {
                covering.join(",")
            },
            self.ids().len(),
            self.dialog_line(),
        )
    }

    /// 这一格账此刻的样子，一行、全 ASCII，供 `--selftest` 那一格印进归档：藏了几枚、
    /// 开着几层、开关是哪一份。写成串而不是把三枚数递出来，是因为它和这所房子其余的
    /// 读数一样要被**同一趟**里的另一格引用（#281 那一族），而一格里只能引一句话。
    fn dialog_line(&self) -> String {
        let hidden: Vec<String> = self.link.hidden().iter().map(|id| id.to_string()).collect();
        format!(
            "enabled={} layers={} hidden={}",
            self.link.is_enabled() as u8,
            self.link.dialogs(),
            if hidden.is_empty() {
                "-".to_string()
            } else {
                hidden.join(",")
            }
        )
    }

    pub fn ids(&self) -> Vec<PinId> {
        self.set.ids()
    }

    pub fn id_at(&self, index: usize) -> PinId {
        self.set.ids().get(index).copied().unwrap_or(0)
    }

    /// §5.9.18 - off the set and off the screen, and Qt is told to forget the
    /// texture rather than hold one for a pin that is gone.
    ///
    /// 那一半「Qt 忘掉纹理」写在 [`Self::retire`] 里，因为「销毁」那一只手要做的是同一件事
    /// （P134 之前它没有桥侧读者，也就没人替它把那六行抽出来 —— 抄第二份就是 #281 那一族：
    /// 一处的 `holes` 漏了，红的是别家那一格）。位图与 GIF 帧**不在**那一格里：关闭把它们
    /// 丢了，销毁把它们搬进 `trash_*`。
    pub fn close(&mut self, ids: &[PinId]) -> usize {
        let n = self.set.close(ids);
        for id in ids {
            self.frames.remove(id);
            self.clips.remove(id);
            self.retire(*id);
        }
        n
    }

    /// 一枚贴图离开屏幕时，桥侧为它结的那笔账：渲染出的那一份、透明扫描的那个缓存、
    /// 推给 Qt 的那枚纹理号，以及 Qt 手里的那张图。
    fn retire(&mut self, id: PinId) {
        self.rendered.remove(&id);
        self.holes.remove(&id);
        self.pushed.remove(&id);
        self.revisions.remove(&id);
        shim::drop_frame(id);
    }

    /// §8.5 - a monitor was unplugged or the resolution changed, so every
    /// remembered position has to be re-checked against the layout that is now
    /// there. Takes the new table as an argument rather than going to the
    /// platform itself because that is the one half of this door a machine with
    /// one fixed display cannot answer: what is under test is whether a pin
    /// outside the new bounds comes back inside them, and no real unplug is
    /// needed to ask it.
    ///
    /// Only the position moves. 裁剪到有效区 is the plan's word for what
    /// [`Desktop::keep_visible`] does to a *window*: it clamps into the union of
    /// what is left and crops no pixel of the picture, so a pin whose pixels did
    /// not change is never re-encoded - nothing here calls [`Self::sync`]. What the
    /// clamp does not do is §8.5's first bullet's 主显示器: the union box keeps dead
    /// space inside it, and a pin parked there is not moved. §10 第 53 项 ④ carries
    /// that as the cell with no spec, instead of this sentence claiming it away.
    pub fn refit_onto(&mut self, desk: Desktop) -> Refit {
        let changed = desk != self.desk;
        self.desk = desk;
        let mut out = Refit {
            changed,
            ..Default::default()
        };
        for p in self.set.items_mut() {
            out.total += 1;
            let before = p.pos_phys;
            if p.move_to(before, &self.desk) != before {
                out.moved += 1;
            }
        }
        out
    }

    /// Hands the pin's current picture to one of the output paths. Taking the
    /// frame by reference rather than cloning it is the point: a 4K pin is 33 MB
    /// and a copy gesture should not pay for it twice.
    ///
    /// Reachable from the gauges since §10 第 43 项 ①: a row that has to say whether the
    /// picture a pin shows is the picture a file holds has to read the frame the pin was
    /// *handed*, and there is no other way to ask without copying megabytes into the row.
    pub(crate) fn with_rendered<R>(&self, id: PinId, f: impl FnOnce(&Frame) -> R) -> Option<R> {
        self.rendered.get(&id).map(f)
    }

    /// §5.5.8 的门要说「按**原截图坐标**重读」有没有真按在那一块上，而那一块除了贴图自己
    /// 没有别人知道：`captured_at` 是 core 那一格的字段，桥不另存一份（#281）。所以门问的
    /// 是这一枚访问器，而不是自己重算一遍矩形。与 [`Self::with_rendered`] 同一形状：读的是
    /// 产品自己那一格，一个像素也不复制。
    pub(crate) fn captured_of(&self, id: PinId) -> Option<PhysRect> {
        self.set.get(id).and_then(|p| p.captured_rect())
    }

    /// §5.9.16 - the pin's own pixels onto the clipboard.
    pub fn copy_image(&self, id: PinId) -> Result<usize, String> {
        let Some(done) = self.with_rendered(id, |frame| {
            platform_windows::clip::write_image(frame)
                .map(|_| (frame.width as usize) * (frame.height as usize))
                .map_err(|e| e.to_string())
        }) else {
            return Err(not_drawn(id));
        };
        done
    }

    /// §5.9.17 - the same copy, with a file reference riding along in the *same*
    /// clipboard transaction, so Explorer's `Ctrl+V` (steps 4-5) has something to make
    /// a real file out of.
    ///
    /// The file is written first and only then promised to the clipboard, because the
    /// promise is the part that can fail: `OpenClipboard` refuses while another process
    /// is mid-copy, and a clipboard that says no must not leave a file named
    /// "clipboard" behind it. On that path the new file is taken back and the
    /// clipboard keeps whatever it already held - §5.9.17's rule is about not deleting
    /// a live reference, never about keeping a dead one.
    ///
    /// The name is `[output] naming_template` / `naming_prefix` / `collision` over again
    /// (rule 2 「文件名应遵守预设命名规则」), through the one owner [`crate::save::quick_to`]
    /// is, with two deliberate differences: the directory is this program's own tagged
    /// corner of the OS temp dir, and `%i` counts *this* gesture rather than the mask's
    /// 快速保存 presses, because the two series are not the same series and a name that
    /// collides across them is a name the collision policy has to settle anyway.
    ///
    /// `%m` and `%t` expand to nothing here: a pin carries neither the monitor it came
    /// from nor the tool that drew its选区 - the same unanswered question §5.5.4's 快速保存
    /// left open, not a new one.
    ///
    /// Returns the pixel count (what [`Self::copy_image`] reports) and the path the
    /// clipboard now names, so the door that called this can say which file a paste
    /// will make.
    pub fn copy_image_with_files(
        &mut self,
        id: PinId,
    ) -> Result<(usize, std::path::PathBuf), String> {
        // Whatever a previous run (or a previous copy whose file the clipboard has since
        // dropped) left in our corner goes first: it is by definition no longer a live
        // reference, and §6.2's 「按策略自动清理」 is this gesture's turn rather than a
        // shutdown hook we do not have yet.
        self.sweep_clip_files();
        let dir = Self::clip_dir();
        let (width, height) = self
            .with_rendered(id, |frame| (frame.width, frame.height))
            .ok_or_else(|| not_drawn(id))?;
        let naming_cfg = crate::settings::naming();
        let export = crate::settings::export();
        let (epoch, offset_min) = platform_windows::clock::wall_clock();
        let facts = naming::NameFacts {
            width,
            height,
            monitor: "",
            tool: "",
        };
        let seq = self.clip_seq + 1;
        let quick = crate::save::Quick {
            dir: &dir,
            template: &naming_cfg.template,
            prefix: &naming_cfg.prefix,
            seq,
            collision: &naming_cfg.collision,
        };
        let written = self.with_rendered(id, |frame| {
            crate::save::quick_to(
                frame,
                naming::LocalStamp::from_epoch(epoch, offset_min),
                facts,
                &quick,
                &export,
                "贴图",
            )
        });
        let path = match written {
            None => return Err(not_drawn(id)),
            Some(Err(crate::save::Refusal::BadName(e))) => return Err(e),
            Some(Err(crate::save::Refusal::Write(e))) => {
                return Err(format!("{e}。贴图还在，可以再复制一次。"))
            }
            Some(Ok((path, _said))) => path,
        };
        self.clip_seq = seq;
        let pixels = self.with_rendered(id, |frame| {
            platform_windows::clip::write_image_with_files(frame, std::slice::from_ref(&path))
                .map(|_| (frame.width as usize) * (frame.height as usize))
                .map_err(|e| e.to_string())
        });
        let pixels = match pixels {
            Some(Ok(counted)) => counted,
            Some(Err(e)) => {
                let _ = std::fs::remove_file(&path);
                return Err(format!("{e}（临时文件已收回，剪贴板上的旧内容照旧）"));
            }
            None => {
                let _ = std::fs::remove_file(&path);
                return Err(not_drawn(id));
            }
        };
        // The promise of this gesture is a file the paste target can actually open, so the
        // block that went out is read back once — the same `CF_HDROP` Explorer will look at,
        // one transaction after we published it. The two ways it can come back are *not* one
        // answer (`None` ≠ `0` 那一族，§10 第 42 项 ②）：
        //   * 读回来了、里面没有我们这一枚：有人在我们之后抢走了剪贴板。这路引用已经是死引用，
        //     文件收回，话照实说。
        //   * 读不回来（剪贴板打不开）：那是**不知道**而不是"没有"。写出去那一笔自己返回了成功，
        //     所以断言仍然成立，文件留在原处等着被贴——把它删掉才是拿一枚读不到当证据。
        if let Ok(held) = platform_windows::clip::read_files() {
            if !held.iter().any(|p| p == &path) {
                let _ = std::fs::remove_file(&path);
                return Err(format!(
                    "剪贴板此刻点名 {} 枚，里面没有这一枚（临时文件已收回）",
                    held.len()
                ));
            }
        }
        // The transaction above ended the previous one, so the file *that* promised is
        // now a dead reference and goes immediately. The new one goes into the register
        // [`Self::sweep_clip_files`] consults.
        let superseded = std::mem::take(&mut self.clip_pending_files);
        for old in superseded {
            if old != path {
                let _ = std::fs::remove_file(&old);
            }
        }
        self.clip_pending_files.push(path.clone());
        Ok((pixels, path))
    }

    /// Delete what our own temp directory holds that the clipboard no longer points at.
    ///
    /// This is the leg that keeps §5.9.17's rule honest: 「临时文件在剪贴板失效前不得删除」
    /// is a prohibition on this program, and the question "has it failed?" is answerable
    /// only by asking the clipboard. The answer this function reports is the one the
    /// caller gets to print, rather than one the code assumes. Two things it will not do:
    ///
    /// - reach outside its own tagged directory, so a sweep can never delete a file the
    ///   user put in `%TEMP%` themselves;
    /// - delete anything at all when the clipboard will not open. A refused
    ///   `OpenClipboard` is not "nothing is referenced"; it is "unknown", and unknown
    ///   keeps every file in place.
    ///
    /// Returns (deleted, still on disk). A file that would not delete counts as still
    /// on disk, which is what the next sweep will find again.
    pub fn sweep_clip_files(&mut self) -> (usize, Vec<std::path::PathBuf>) {
        let dir = Self::clip_dir();
        let on_disk = match std::fs::read_dir(&dir) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .collect::<Vec<_>>(),
            // No directory means no files, and the register that names them is stale.
            Err(_) => {
                self.clip_pending_files.clear();
                return (0, Vec::new());
            }
        };
        let held = match platform_windows::clip::read_files() {
            Ok(files) => files,
            Err(_) => return (0, on_disk),
        };
        let mut deleted = 0usize;
        let mut kept = Vec::new();
        for path in on_disk {
            if held.contains(&path) {
                kept.push(path);
            } else if std::fs::remove_file(&path).is_ok() {
                deleted += 1;
            } else {
                kept.push(path);
            }
        }
        self.clip_pending_files.retain(|path| held.contains(path));
        (deleted, kept)
    }

    /// §5.9.17 的临时目录: the OS temp dir under our own tag. The tag is what makes the
    /// sweep safe - it only ever lists a directory this program created and owns.
    ///
    /// `pub(crate)` 是给 `pin_check` 的 `copy_rows` 的：那一格判「这句话点名的文件在不在我们
    /// 自己那一枚目录下」，用的是这一枚函数而不是档头里再抄一遍 `%TEMP%\falconshot-clip`。
    pub(crate) fn clip_dir() -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push("falconshot-clip");
        dir
    }

    /// [`Self::remind_before_save`]'s wording without the assignment: what this encoder is
    /// about to cost this pin, in the future tense PRD 447 asks for.
    ///
    /// The empty string is one answer with three comings - a format that carries alpha, a
    /// pin that was never drawn, and a name that decides no format - which is why it reads
    /// as "this door loses no transparency" and never as "no reminder was due". The
    /// sentence itself has one owner ([`crate::save::reminder`]) and reaches the mask's
    /// three doors unchanged, because only the subject differs: 贴图, not 选区.
    fn remind_with(&self, id: PinId, path: &str, chosen: &str, export: &EncodeOptions) -> String {
        self.with_rendered(id, |frame| {
            crate::save::reminder(frame, path, chosen, export, "贴图").unwrap_or_default()
        })
        .unwrap_or_default()
    }

    /// [`Self::save_image`]'s first rung on its own, so the order is *callable*: a gauge
    /// can ask what this door said at the moment the file it warns about was not on disk
    /// yet. It writes nothing, and the write that follows it is the product's own
    /// [`Self::save_image`] - a reminder is not a confirmation.
    pub fn remind_before_save(&mut self, id: PinId, path: &str, chosen: &str) -> String {
        self.pre_notice = self.remind_with(id, path, chosen, &crate::settings::export());
        self.pre_notice.clone()
    }

    /// §5.5.5 的贴图侧 - the pin out to a file, through the same owner the mask's 另存为 uses
    /// ([`crate::save`]), so the two doors read `[output] jpeg_quality`, answer in the
    /// same Chinese sentence and name the pixels a format without alpha threw away.
    /// The extension picks the format, so `core::encode`'s ICO-downscale and
    /// JPG-flatten rules apply on the way; a name without one takes the format the
    /// dialog was showing (`chosen`), and only when that offers nothing the configured
    /// one - which is the half P35 fixed, widened by §10 第 40 项 ② because PRD §5.5.5
    /// has the user choosing 格式 in that dialog too.
    ///
    /// The first rung is [`Self::remind_before_save`]'s, in the same order and for the same
    /// reason as the mask's `save_out`: PRD 447 says 背景**将**被填充, and
    /// [`crate::save::write`] turns that into 已填成白色 the moment it returns. §10 第 46 项
    /// ③ registered this door as the one that only had the after half; this is the before
    /// half, and the write still happens.
    pub fn save_image(&mut self, id: PinId, path: &str, chosen: &str) -> Result<String, String> {
        let export = crate::settings::export();
        self.pre_notice = self.remind_with(id, path, chosen, &export);
        let Some(done) = self.with_rendered(id, |frame| {
            match crate::save::write(frame, path, &export, "贴图", chosen) {
                Ok(said) => Ok(said),
                Err(crate::save::Refusal::BadName(e)) => Err(e),
                Err(crate::save::Refusal::Write(e)) => {
                    Err(format!("{e}。贴图还在，可以再保存一次。"))
                }
            }
        }) else {
            return Err(not_drawn(id));
        };
        done
    }

    /// §5.8.3 / §10 第 43 项 ① - a file that is already on disk becomes a pin.
    ///
    /// Until P43 the only way a picture living in a file could reach the screen was the
    /// clipboard: [`Self::add_from_clipboard`] took a `Files` payload and decoded its
    /// first entry (`encode::decode_file(first)`). That made 存完再钉图 an accident - the
    /// user had to copy the file, and it made §5.8.3's own "选择图片文件 → 系统读取图片并
    /// 创建贴图" unreachable without a trip through the shell.
    ///
    /// Reading the file is the whole of the difference from [`Self::add`], and the reason
    /// to do it here rather than at each door is the second half of PRD line 910: a
    /// damaged or unsupported file "应显示错误信息". Spelled once, both doors answer the
    /// same way, and neither one gets to forward `EncodeError`'s English.
    pub fn add_from_file(&mut self, path: &str, at: PhysPoint) -> Result<PinId, String> {
        let frame = match encode::decode_file(std::path::Path::new(path)) {
            Ok(f) => f,
            Err(e) => return Err(read_refusal(path, &e)),
        };
        let id = self.add(frame, at)?;
        self.attach_clip(id, std::path::Path::new(path));
        Ok(id)
    }

    /// §5.9.14: hand a pin the frames its own file carries. Both doors that read a
    /// path call this - 选择图片文件 and a file copied in Explorer - so a GIF
    /// animates whichever way it arrived.
    ///
    /// A clip that will not read is noted, not a refused paste: the first frame is
    /// already on screen and the pin keeps showing it until the user pastes the
    /// file again. A file that is not a GIF at all answers "no clip" without
    /// decoding, which is why pasting a PNG does not pay for a second read of it.
    fn attach_clip(&mut self, id: PinId, path: &std::path::Path) {
        let clip = match falcon_core::gif::decode_file(path) {
            Ok(clip) => clip,
            Err(e) => {
                self.note(format!("{}: {e}", path.display()));
                None
            }
        };
        if let Some(clip) = clip {
            let count = clip.frame_count();
            self.clips.insert(id, clip);
            if let Some(p) = self.set.get_mut(id) {
                p.set_gif(count);
            }
        }
    }

    /// §5.8.2 - whatever the user copied becomes a pin. Deciding *what it was* is
    /// `core::clip::classify`'s job, already tested; this moves bytes for the two shapes
    /// that are already pictures and lays out a card for the three that are words.
    pub fn add_from_clipboard(&mut self, at: PhysPoint) -> Result<PinId, String> {
        let contents = platform_windows::clip::read().map_err(|e| e.to_string())?;
        if contents.is_empty() {
            return Err("剪贴板是空的".into());
        }
        let payload = {
            let facts = contents.facts();
            falcon_core::clip::classify(&facts).map_err(|e| e.to_string())?
        };
        // §5.9.14: the file door is not only 选择文件. A GIF copied in Explorer is
        // the same file with the same frames, so the path is kept long enough for
        // the clip to be read from it once the pin exists.
        let pasted_file = match &payload {
            (falcon_core::clip::ClipKind::Files, falcon_core::clip::ClipPayload::Files(paths)) => {
                paths.first().cloned()
            }
            _ => None,
        };
        let frame = match payload {
            (falcon_core::clip::ClipKind::Image, falcon_core::clip::ClipPayload::Image(f)) => f,
            (falcon_core::clip::ClipKind::Files, falcon_core::clip::ClipPayload::Files(paths)) => {
                let first = paths.first().ok_or("剪贴板里的文件列表是空的")?;
                encode::decode_file(first).map_err(|e| e.to_string())?
            }
            (falcon_core::clip::ClipKind::Colour, falcon_core::clip::ClipPayload::Colour(rgba)) => {
                // §5.8.6 step 3: the swatch and the value it stands for are one card, not
                // a 64×64 square. The 背景颜色 the user set in `[card]` is what the copied
                // colour replaces here; width, 字号 and 字体 still come from the same file.
                let mut card = crate::settings::card_cfg();
                card.background =
                    falcon_core::colors::format(rgba, falcon_core::colors::ColorFormat::HexUpper);
                card_picture(&colour_value(rgba), &card)?
            }
            (falcon_core::clip::ClipKind::Text, falcon_core::clip::ClipPayload::Text(text)) => {
                card_picture(&text, &crate::settings::card_cfg())?
            }
            (falcon_core::clip::ClipKind::Html, falcon_core::clip::ClipPayload::Html(html)) => {
                // §5.8.2's 保留基本格式 is the line structure: `html_text` keeps the blocks
                // the sanitizer allowed and drops the styling along with the markup.
                card_picture(
                    &falcon_core::clip::html_text(&html),
                    &crate::settings::card_cfg(),
                )?
            }
            (_, falcon_core::clip::ClipPayload::Empty) => Err("剪贴板没有可贴图的内容")?,
            _ => Err(
                "剪贴板里的这一种内容还贴不成图——能贴的是图片、图片文件、纯文本、HTML 和颜色值"
                    .to_string(),
            )?,
        };
        let id = self.add(frame, at)?;
        if let Some(path) = pasted_file {
            self.attach_clip(id, &path);
        }
        Ok(id)
    }

    fn note(&mut self, message: String) {
        self.problems.push(message);
    }
}

/// §8.1: the paste path words its own refusals, and the two are different answers - one
/// names what to do about the paste, the other names the key holding the string nobody
/// could read.
fn card_refusal(e: falcon_core::card::CardError) -> String {
    use falcon_core::card::CardError as E;
    match e {
        E::TooLarge(w, h) => format!(
            "这段文字排出来是 {w}×{h} 像素，比一张贴图能装的还大——可以分开几次贴，或在 [card] 里把字号调小。"
        ),
        E::Background(value) => format!(
            "[card] 里的 background = {value} 不是颜色值，卡片没有画——改成 #RRGGBB 再贴一次。"
        ),
    }
}

/// §5.8.5 / §5.8.6: the one place a card is asked for, so the three word-shaped pastes
/// (纯文本、HTML 文本、颜色值) cannot disagree about what 宽度、字号、字体 mean.
///
/// DirectWrite answers the measuring, the same provider the capture mask lays its own text
/// with. Where a machine has no font to ask, [`falcon_core::card`] paints the background
/// and nothing else rather than inventing a line height.
fn card_picture(text: &str, card: &falcon_core::config::Card) -> Result<Frame, String> {
    falcon_core::card::render(text, card, &platform_windows::glyphs::DirectWrite)
        .map_err(card_refusal)
}

/// §5.8.6 step 3's 颜色值: six digits when the copied colour is opaque, eight when it is
/// not. The alpha is written rather than applied - a swatch painted with `#00000000` is a
/// picture of nothing at all, and the value line is what the user reads the colour off.
fn colour_value(rgba: [u8; 4]) -> String {
    use falcon_core::colors::{format, ColorFormat};
    if rgba[3] == 0xff {
        format(rgba, ColorFormat::HexUpper)
    } else {
        format(rgba, ColorFormat::HexRgba)
    }
}

/// What went wrong with a file, said the way the user reads it.
///
/// PRD line 910 asks for a message on both shapes, and they are different answers: one says
/// the file is broken, the other says this program does not read that kind of file. The
/// decoder cannot tell them apart - an unrecognised extension and a damaged file both
/// arrive as `Codec`, because `encode::decode_file` hints the format from the extension and
/// falls back to guessing from the bytes - so the name is asked as well as the error.
/// The file's own path is the one piece that travels through as typed: it is what the user
/// can find in the dialog again.
fn read_refusal(path: &str, e: &encode::EncodeError) -> String {
    use encode::EncodeError as E;
    let readable = encode::IMPORT_FORMATS
        .iter()
        .map(|f| f.ext())
        .collect::<Vec<_>>()
        .join("、");
    let named = std::path::Path::new(path)
        .extension()
        .and_then(|x| x.to_str())
        .and_then(encode::Format::from_ext);
    match e {
        E::Io(_) => format!("{path} 打不开：文件读不进来（可能已被移走、正被占用，或没有权限）"),
        E::TooLarge(w, h) => format!("{path} 太大读不进来（{w}×{h} 像素）"),
        E::Codec(_) | E::Unsupported(_) => match named {
            None => format!("{path} 不是本程序会读的图片：用 {readable}。"),
            Some(_) => format!("{path} 读不出图片：文件可能已损坏。"),
        },
        E::Frame(_) | E::TransparencyLost(_) => format!("{path} 钉不上：这张图拼不出像素"),
    }
}

/// What both output doors say about a pin that has no picture yet.
///
/// One sentence rather than two copies because it is a promise to the user, not a
/// log line: [`crate::session`] wraps it under `复制失败：` and `保存失败：`, and an
/// English clause inside either prefix is what §10 第 37 项 ① measured for the first
/// time in P37. It names 贴图 so the wrapping reads as one Chinese sentence, and the
/// `--pin` gauge judges exactly those two properties (contains 贴图, no English).
fn not_drawn(id: PinId) -> String {
    format!("贴图 {id} 还没有画过，没有可输出的像素")
}

// ---------------------------------------------------------------- the session

static STATE: OnceLock<Mutex<PinState>> = OnceLock::new();

pub fn state() -> &'static Mutex<PinState> {
    STATE.get_or_init(|| Mutex::new(PinState::new()))
}

/// Run one thing against the live set. Every caller is on the GUI thread, so the
/// lock is a formality - but a panic must not take the app down, which is why
/// `into_inner` is used rather than `unwrap`.
pub fn with<R>(f: impl FnOnce(&mut PinState) -> R) -> R {
    let mut guard = state().lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut guard)
}

/// The screen layout pins are clamped to: every monitor the platform enumerates,
/// in physical pixels and each with its own ratio.
///
/// This is where a pin's `dpr` comes from (plan §10 第 52 项 ①). It cannot come
/// from QML's `Screen` attached object, which only resolves once the window is
/// already placed - and placing the window is what needs the ratio. The
/// enumeration reads the same screens Qt does, but in the physical space a
/// `PhysRect` is, and costs no pixels (plan §6.1 "几何只用物理像素").
///
/// If the enumeration itself fails, Qt's primary answers instead - after its DIP
/// rect is converted by that screen's own ratio, because a `PhysRect` written from
/// raw DIP is the unit leak the plan forbids. The other monitors being gone is
/// reported, not swallowed: a pin clamped to one screen behaves differently on two.
fn desktop_from_platform(problems: &mut Vec<String>) -> Desktop {
    match capture::monitor_table() {
        Ok(ms) if !ms.is_empty() => {
            return Desktop::from_monitors(
                ms.iter().map(|m| Monitor::new(m.bounds, m.scale)).collect(),
            );
        }
        Ok(_) => problems.push("显示器枚举返回空表，贴图只能夹在主屏上".into()),
        Err(e) => problems.push(format!("显示器枚举失败（{e}），贴图只能夹在主屏上")),
    }
    let scale = Scale::of(shim::device_pixel_ratio());
    let (px, py, pw, ph) = shim::primary_bounds();
    let rect = scale.rect_to_phys(&DipRect {
        x: px as f64,
        y: py as f64,
        w: pw.max(1) as f64,
        h: ph.max(1) as f64,
    });
    Desktop::from_monitors(vec![Monitor::new(rect, scale)])
}

/// The 提示 the plan owes §8.5 (「屏幕变化：分辨率/插拔时裁剪到有效区并提示」 - PRD
/// §8.5 itself asks for the two behaviours and for no words), and the only place
/// that words it - so a test can grade the sentence without a display. It says how
/// many pins moved and names no monitor it did not read.
fn refit_notice(moved: usize) -> String {
    format!("屏幕变化：已把 {moved} 张贴图移回屏幕内")
}

/// Whether a screen change is worth a line on the screen at all. `None` is the
/// ordinary answer: a change that moved nothing has told the user nothing about
/// their pins, and a sentence about a screen would be read as one about a picture.
/// Kept apart from [`desktop_changed`] because the platform door that produces a
/// [`Refit`] cannot be run on a machine with one fixed display, while this can -
/// which is also why the two cases are graded by the counts rather than by a run.
fn notice(done: Refit) -> Option<String> {
    (done.moved > 0).then(|| refit_notice(done.moved))
}

/// The §8.5 door: re-read the layout the platform has *now*, refit every live pin
/// onto it, and answer with the sentence the user should see.
///
/// The enumeration is re-run here rather than served from the cached table,
/// because the stale table is precisely the thing that went wrong.
pub fn desktop_changed() -> Option<String> {
    let mut problems = Vec::new();
    let desk = desktop_from_platform(&mut problems);
    let done = with(|s| {
        s.problems.append(&mut problems);
        s.refit_onto(desk)
    });
    notice(done)
}

/// How the pin clamp sees the desktop, on one line a probe can quote: the monitors
/// as they were enumerated, then the union they clamp against.
fn clamp_line(desk: &Desktop) -> String {
    let monitors = desk
        .monitors
        .iter()
        .map(|m| {
            format!(
                "{}x{}@{},{} dpr={:.2}",
                m.rect.w,
                m.rect.h,
                m.rect.x,
                m.rect.y,
                m.dpr.ratio()
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    format!(
        "clamp [{}] bounds={}x{}@{},{}",
        monitors, desk.bounds.w, desk.bounds.h, desk.bounds.x, desk.bounds.y
    )
}

/// [`clamp_line`] for the live set, for a reader that is not already inside the state.
pub fn pin_desktop_summary() -> String {
    with(|s| clamp_line(&s.desk))
}

pub fn desktop_summary() -> String {
    let (dx, dy, dw, dh) = shim::desktop_bounds();
    let (px, py, pw, ph) = shim::primary_bounds();
    format!(
        "desktop={dx},{dy} {dw}x{dh} primary={px},{py} {pw}x{ph} dpr={:.2} {}",
        shim::device_pixel_ratio(),
        platform_windows::desktop::station_summary()
    )
}

pub fn window_report() -> String {
    shim::window_report()
}

/// How many screen signals the §8.5 watch is connected to. Zero is a wiring answer,
/// not a screen answer: it means `main.qml` never installed the watch. The count is
/// printed by the door that reads it rather than asserted here, because it is a
/// function of how many screens a machine has - two per screen plus the two that
/// watch the set itself. This machine answered six on 2026-10-11 with both monitors
/// enumerated and four on 2026-10-12 after the second one stopped being one, which
/// is the number telling us the desktop, not the build, is what it describes. What
/// no run can grade from a number is the half of §8.5 that unplugs a monitor while
/// watching.
pub fn screen_watch_connections() -> i32 {
    shim::screen_watch_connections()
}

/// Qt's own complaints - the QML compiler's errors among them - kept where a
/// headless run can print them.
pub fn install_message_capture() {
    shim::install_message_capture();
}

pub fn messages() -> String {
    shim::messages()
}

/// Set by `main.qml` when its root finishes constructing. The probe reads it,
/// because `topLevelWindows=0` has two very different causes: the document never
/// loaded, or it loaded and made no windows.
static QML_LOADED: AtomicBool = AtomicBool::new(false);

pub fn mark_qml_loaded() {
    QML_LOADED.store(true, Ordering::Relaxed);
    stamp("qml");
}

pub fn qml_loaded() -> bool {
    QML_LOADED.load(Ordering::Relaxed)
}

/// Whether this run is allowed to put the hint window on the screen at all.
///
/// Written once by `main`, from the flags it has already parsed - the resident app and
/// `--hint` are the two runs where the surface exists, and every measuring door keeps it
/// away. The reason is not tidiness: a hint appears whenever something writes `status`,
/// and the gauge legs write it on purpose (§10 第 41 项 ② grades a pair of sentences
/// across a click). Meanwhile `pin_check::pin_window` chooses *the visible top-level that
/// overlaps the pin's rect*, so a toast that came up in a `--pin` run would be a second
/// candidate for the window whose pixels that row is about. A measurement must not depend
/// on another part of the program being polite, so the door that could disturb it is shut
/// by one predicate rather than by the leg order.
static HINT_SURFACE: AtomicBool = AtomicBool::new(false);

pub fn set_hint_surface(on: bool) {
    HINT_SURFACE.store(on, Ordering::Relaxed);
}

pub fn hint_surface() -> bool {
    HINT_SURFACE.load(Ordering::Relaxed)
}

/// Cold-path stamps: milliseconds since the first one, per named stage.
///
/// `--mask` prints one `cold of` figure and §10-14 ② has to reduce it, but a single
/// number cannot say which part a pre-warm can even reach. Creating the QML engine
/// and compiling the document is work a resident process has already finished before
/// any hot key is pressed; the freeze, the texture hand-off and the first painted
/// frame have not. Stamping both ends of each stage is what turns "1078 ms, reduce
/// it" into "these 700 ms disappear by itself, these 378 ms are the ones to engineer".
static CLOCK: OnceLock<Instant> = OnceLock::new();
static STAMPS: Mutex<Vec<(&'static str, u64)>> = Mutex::new(Vec::new());

/// Record `name` once, at its elapsed time. A second call with the same name is
/// ignored on purpose: these mark the *first* time a stage happened, which is the
/// only thing a cold path has.
pub fn stamp(name: &'static str) {
    let clock = CLOCK.get_or_init(Instant::now);
    let ms = clock.elapsed().as_millis() as u64;
    let mut v = STAMPS.lock().unwrap_or_else(PoisonError::into_inner);
    if !v.iter().any(|(n, _)| *n == name) {
        v.push((name, ms));
    }
}

/// Milliseconds between two stamps, or `None` when either is missing. Missing is
/// never reported as `0`: a stage that did not run must not read as one that was free.
pub fn between(a: &str, b: &str) -> Option<u64> {
    let v = STAMPS.lock().unwrap_or_else(PoisonError::into_inner);
    let at = |n: &str| v.iter().find(|(x, _)| *x == n).map(|(_, ms)| *ms);
    Some((at(b)? as i64 - at(a)? as i64).max(0) as u64)
}

/// `name=ms` for every stamp taken so far, in the order they were taken.
pub fn stamp_line() -> String {
    let v = STAMPS.lock().unwrap_or_else(PoisonError::into_inner);
    v.iter()
        .map(|(n, ms)| format!("{n}={ms}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn quit_after(ms: i32) {
    shim::quit_after(ms);
}

/// The `--mask` gauge's second and later rounds.
///
/// `main.rs` reads the command line and writes this before the QML engine exists;
/// `Session` pulls it into qproperties from `Component.onCompleted`, because the
/// qobject is created by QML and Rust has no handle to push through beforehand.
/// Only a QML `Timer` can act once the event loop is running, and the question the
/// rounds answer - what a hot key really costs - cannot be asked before then.
#[derive(Default)]
pub struct WarmPlan {
    /// Rounds still owed. Counts down as they run, so the timer stops itself.
    pub left: i32,
    pub step_ms: i32,
    pub shader: bool,
    pub rounds: Vec<String>,
}

static WARM: OnceLock<Mutex<WarmPlan>> = OnceLock::new();

pub fn warm<R>(f: impl FnOnce(&mut WarmPlan) -> R) -> R {
    let mut guard = WARM
        .get_or_init(|| Mutex::new(WarmPlan::default()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    f(&mut guard)
}

/// Where a new pin goes: beside the pointer, because that is where the user was
/// looking when they pressed the key, stepped aside 8 px so the picture does not
/// hide under the arrow. The state machine then clamps it to the desktop, which is
/// what keeps a paste near a screen edge partly visible rather than off-screen.
///
/// The centre of the primary screen is the fallback, and only for a build that has
/// no pointer to ask (a non-Windows compile of this workspace).
fn next_drop_point() -> PhysPoint {
    if let Some(p) = platform_windows::proc::cursor_pos() {
        return PhysPoint::new(p.x + 8, p.y + 8);
    }
    let (x, y, w, h) = shim::primary_bounds();
    let step = with(|s| s.ids().len()).min(24) as i32;
    PhysPoint::new(x + w / 2 + step * 16, y + h / 2 + step * 16)
}

/// §5.8.2 - the clipboard becomes a pin. Shared by the QML menu path and by
/// `--paste`, which exists so this can be verified on a real machine today rather
/// than only once the global hotkey lands in M5.
pub fn paste_from_clipboard() -> Result<PinId, String> {
    with(|s| s.add_from_clipboard(next_drop_point()))
}

/// §5.9.1: a new pin arrives roughly where the user is looking, and the desktop
/// it is clamped against is Qt's own geometry rather than a guess.
pub fn add_demo(width: u32, height: u32) -> Result<PinId, String> {
    let (_x, _y, pw, ph) = shim::primary_bounds();
    let at = PhysPoint::new(
        pw.saturating_sub(width as i32) / 2,
        ph.saturating_sub(height as i32) / 2,
    );
    with(|s| s.add(demo_frame(width, height), at))
}

/// A picture to pin when nothing else has produced one yet: a checkerboard with a
/// transparent margin, so §5.9's alpha promise is visible before the capture
/// service exists.
pub fn demo_frame(width: u32, height: u32) -> Frame {
    let w = width.clamp(4, 4096);
    let h = height.clamp(4, 4096);
    let mut f = Frame::filled(w, h, [0x11, 0x22, 0x33, 0xff]).expect("frame");
    let cell = (w.min(h) / 6).max(1);
    for y in 0..h {
        for x in 0..w {
            if x < 2 || y < 2 || x + 2 >= w || y + 2 >= h {
                f.set(x, y, [0, 0, 0, 0]);
            } else if (x / cell + y / cell).is_multiple_of(2) {
                f.set(x, y, [0xe8, 0x6c, 0x2c, 0xff]);
            }
        }
    }
    f
}

/// Fully transparent: whatever the window shows is the alpha background, so a
/// board that was not painted cannot hide behind a picture.
fn transparent_frame(width: u32, height: u32) -> Frame {
    Frame::filled(width, height, [0x00, 0x00, 0x00, 0x00]).expect("frame")
}

// ------------------------------------------------------------------- selftest

/// The render → encode → store → decode round trip, checked without showing a
/// window. Every step is one the GUI path uses for real, so this fails when the
/// pixel pipeline is broken rather than only when a binding is missing.
pub fn selftest() -> (bool, String) {
    let mut out = Vec::new();
    let mut ok = true;

    {
        let mut check = |label: &str, passed: bool, detail: String| {
            ok &= passed;
            out.push(format!(
                "{label:<34} {:<4} {detail}",
                if passed { "PASS" } else { "FAIL" }
            ));
        };

        // Two questions in one row, because answering only the first still shows
        // nobody a pin: is the desktop measurable, and is it the desktop the user
        // is looking at. A process launched from a service or a scheduled task
        // measures a perfect geometry on a station nobody can see.
        let (_, _, dw, dh) = shim::desktop_bounds();
        check(
            "desktop geometry is real",
            dw > 0 && dh > 0 && desktop_summary().contains("station=WinSta0"),
            desktop_summary(),
        );

        // A second, different reading of the same desktop (§10 第 52 项 ①/②): the
        // table a pin is clamped against. It has to be the platform's enumeration -
        // every monitor, physical pixels, one ratio each - because it is now also
        // the only answer to what ratio a window on that monitor is sized with.
        let listed = capture::monitor_table().unwrap_or_default();
        let (clamp, matched) = with(|s| {
            let matched = s
                .desk
                .monitors
                .iter()
                .zip(listed.iter())
                .filter(|(m, e)| m.rect == e.bounds && m.dpr == e.scale)
                .count();
            (clamp_line(&s.desk), matched)
        });
        check(
            "pin clamp is the physical table",
            !listed.is_empty() && matched == listed.len(),
            format!("{clamp} matched={matched} of {}", listed.len()),
        );

        let src = demo_frame(64, 32);
        let id = match with(|s| s.add(src.clone(), PhysPoint::new(10, 10))) {
            Ok(id) => id,
            Err(e) => return (false, format!("could not add a pin: {e}")),
        };
        check(
            "first frame decodes",
            !shim::self_check(id).contains("null=1"),
            shim::self_check(id),
        );

        let after_add = with(|s| s.view_data(id));
        check(
            "adding pushed one texture",
            after_add.revision == 1,
            format!("revision={}", after_add.revision),
        );

        // §5.17 的命令行接口第一次进 `--selftest`。判据是 [`crate::cli::check`] 那张 argv
        // 判决表本身——语法、三条状态码规则（成功 0 / 失败 1 / 参数错 3）与帮助文本，headless
        // 一次跑完，而不是"这格没崩"（#274 那一族）。像素与遮罩交互仍归真机 A/B 门；这一行证的
        // 是文法与 code 映射，契约住在模块里而非 `fn main`（§10 第 40 项 ③：`main` 对
        // `cargo test` 不可见，所以判决得能被问出来）。
        let (cli_check, cli_said) = crate::cli::check();
        check(
            "§5.17 cli contract headless",
            cli_check == Check::Pass,
            cli_said,
        );

        // §5.5.11 步骤 3/4 —— 第 176 项剩下的那一半：一扇文件保存对话框的出现与关闭，走进
        // 状态机之后确实按 `covering` 的相交判藏过、又放回过。九手一条腿跑完，判据是**每一手
        // 各自的行为**，不是"这格没崩"（#274 那一族）。
        //
        // 矩形现读自 [`PinState::pin_rects`]：写死一枚矩形会让这条腿去判一张根本不存在的窗。
        // 第 8 手是运行期**普通**的那一臂 —— Qt 把 `FileDialog` 标成可见要比 Win32 真造出那扇
        // `#32770` 早约 400 ms（`PinWindow.qml` 头顶记着那枚测量），所以真事件到达时读不到
        // 矩形，那一臂藏**全部**：「可能遮挡」最保守的读法。
        let (link_ok, link_said) = with(|s| {
            let Some((pin, r)) = s.pin_rects().first().copied() else {
                return (false, "集里没有贴图可藏".to_string());
            };
            if r.w == 0 || r.h == 0 {
                return (
                    false,
                    format!("贴图 {pin} 的窗口零面积，藏它判不出任何事: {r}"),
                );
            }
            // 就在它右边 8 px：相交判在这里必须是"没压住"，否则第 2 手与第 4 手是同一问。
            let miss = PhysRect::new(r.x + r.w as i32 + 8, r.y, r.w, r.h);
            let mut bad: Vec<&str> = Vec::new();
            // 1 手：步骤 1 那一格关着 ⇒ 事件压根没进这一层，一枚也不藏。
            if !s.dialog_opened(false, Some(r)).is_empty() || !s.view_data(pin).live {
                bad.push("off");
            }
            // 2、3 手：开着、可这张框压不住它 ⇒ 也不许藏。"可能遮挡"不是"全部藏掉"。
            if !s.dialog_opened(true, Some(miss)).is_empty() || !s.view_data(pin).live {
                bad.push("miss");
            }
            if !s.dialog_closed().is_empty() || !s.view_data(pin).live {
                bad.push("miss-close");
            }
            // 4 手：压住了 ⇒ 就是这一枚，且窗口跟着不 live。
            if s.dialog_opened(true, Some(r)) != vec![pin] || s.view_data(pin).live {
                bad.push("cover");
            }
            // 5、6、7 手：叠第二层不重复藏；第一层关掉还不放回；最后一层关掉才放回。
            if !s.dialog_opened(true, Some(miss)).is_empty() || s.view_data(pin).live {
                bad.push("layer-2");
            }
            if !s.dialog_closed().is_empty() || s.view_data(pin).live {
                bad.push("first-close");
            }
            if s.dialog_closed() != vec![pin] || !s.view_data(pin).live {
                bad.push("last-close");
            }
            // 8、9 手：读不到矩形的那一臂 ⇒ 藏集里全部，关闭 ⇒ 按同一份名单放回。
            let all = s.dialog_opened(true, None);
            if all != s.ids() || s.view_data(pin).live {
                bad.push("no-rect");
            }
            if s.dialog_closed() != all || !s.view_data(pin).live {
                bad.push("no-rect-close");
            }
            let said = s.dialog_line();
            (
                bad.is_empty(),
                if bad.is_empty() {
                    format!("pin={pin} rect={r} {said} bad=none")
                } else {
                    format!("pin={pin} rect={r} {said} bad={}", bad.join(","))
                },
            )
        });
        check("dialog linkage hides and restores", link_ok, link_said);
        check(
            "the transparent margin survives",
            shim::pixel(id, 0, 0) == "0,0,0,0",
            format!("(0,0)={}", shim::pixel(id, 0, 0)),
        );

        // The whole reason the state machine is authoritative: zoom is geometry.
        let _ = with(|s| s.edit(id, |p, _, _| p.set_zoom(300)));
        let after_zoom = with(|s| s.view_data(id));
        check(
            "zoom re-encodes nothing",
            after_zoom.revision == after_add.revision && after_zoom.size_w == 192,
            format!(
                "revision={} window={}x{} at {},{}",
                after_zoom.revision,
                after_zoom.size_w,
                after_zoom.size_h,
                after_zoom.pos_x,
                after_zoom.pos_y
            ),
        );

        // Rotation swaps the shown rectangle, so the bitmap has to change.
        let _ = with(|s| s.edit(id, |p, _, _| p.rotate_cw()));
        check(
            "rotation re-encodes",
            shim::self_check(id).contains("w=32 h=64"),
            format!(
                "{} revision={}",
                shim::self_check(id),
                with(|s| s.view_data(id).revision)
            ),
        );

        // Effects are content, so they cross too - and the decode must hold.
        let _ = with(|s| {
            s.edit(id, |p, _, _| {
                p.toggle_grayscale();
                p.toggle_invert();
            })
        });
        check(
            "grayscale + invert decodes",
            !shim::self_check(id).contains("null=1"),
            shim::self_check(id),
        );

        // §5.9.15: the board is baked into the texture, not left for QML to invent.
        with(|s| s.cfg.alpha_bg = AlphaBg::CheckerLight);
        let board = match with(|s| s.add(transparent_frame(16, 1), PhysPoint::new(20, 20))) {
            Ok(id) => id,
            Err(e) => return (false, format!("could not add the board pin: {e}")),
        };
        with(|s| s.cfg.alpha_bg = AlphaBg::Transparent);
        check(
            "the board paints see-through",
            shim::pixel(board, 0, 0) == "212,212,212,255"
                && shim::pixel(board, 8, 0) == "239,239,239,255",
            format!(
                "(0,0)={} (8,0)={}",
                shim::pixel(board, 0, 0),
                shim::pixel(board, 8, 0)
            ),
        );

        // A crop is content; the texture shrinks with it.
        let cropped = with(|s| s.edit(board, |p, _, _| p.crop(&PhysRect::new(0, 0, 4, 1))));
        check(
            "crop re-encodes smaller",
            matches!(cropped, Some(Ok(()))) && shim::self_check(board).contains("w=4 h=1"),
            shim::self_check(board),
        );

        // §5.9.11: what the window shows is not what the picture is.
        let thumb = with(|s| {
            let _ = s.edit(board, |p, _, _| {
                p.set_free_thumbnail(&PhysRect::new(2, 0, 1, 1))
            });
            s.view_data(board)
        });
        check(
            "a free thumbnail shows its region",
            shim::self_check(board).contains("w=1 h=1") && thumb.size_w == 1,
            format!(
                "{} window={}x{}",
                shim::self_check(board),
                thumb.size_w,
                thumb.size_h
            ),
        );

        // §5.8.5 / §5.8.2 / §5.8.6: the card the paste door draws is a picture with ink
        // in it, laid out by this machine's own font. Three answers in one row - it
        // rendered, it is not a flat background, and it came back out of the set. §9.1
        // keeps the numbers here and the pixels out of the log.
        let (card_ok, card_said) =
            match card_picture("文本卡片 one two three", &crate::settings::card_cfg()) {
                Err(e) => (false, e),
                Ok(frame) => {
                    let (w, h) = (frame.width, frame.height);
                    let flat = frame.is_uniform().is_some();
                    match with(|s| s.add(frame, PhysPoint::new(30, 30))) {
                        Err(e) => (false, e),
                        Ok(card_id) => {
                            let dropped = with(|s| s.close(&[card_id]));
                            (
                                !flat && dropped == 1,
                                format!("card {w}x{h} flat={flat} closed={dropped}"),
                            )
                        }
                    }
                }
            };
        check("a text card is not flat", card_ok, card_said);

        // The gate a file passes on load is the gate a write passes (§10 第 170 项):
        // a widget that stores 500 for a key the file can only carry as 96 gets the
        // 96 back, and the correction comes along named instead of vanishing.
        let mut probe = Config::default();
        let (gate_ok, gate_said) = match probe.set_str("card.font_size", "500") {
            Err(e) => (false, format!("refused: {e}")),
            Ok(corrections) => {
                let read_back = probe.get_str("card.font_size").unwrap_or_default();
                (
                    probe.card.font_size == falcon_core::annotation::FONT_MAX
                        && read_back == falcon_core::annotation::FONT_MAX.to_string()
                        && corrections.iter().any(|c| c.contains("card.font_size")),
                    format!(
                        "stored={} read={} corrections={} first={}",
                        probe.card.font_size,
                        read_back,
                        corrections.len(),
                        corrections.first().cloned().unwrap_or_default(),
                    ),
                )
            }
        };
        check("config write runs the gate", gate_ok, gate_said);

        // §5.20.1 与 §5.20.2 的那一格，紧挨上面那一格放：两行问的是同一件事的两头——上面那行问
        // 「门把打进去的值改成了什么」，这一行问「哪些值有资格被改、哪些值改了要重启」。四格全是
        // 纯读数（出厂默认那份 config 的键清单、名单上的归属、需重启标记与那份名单的逐字比对、
        // 语法闸的三臂），一件物事也不写盘、也不看屏幕，所以它长在**每一趟** `--selftest` 都走得到
        // 的地方，而不是长在下面那条要真桌子的梯子里：一句不必靠屏幕的判据如果被屏幕挡住，
        // 那一趟就谁也没问过它（这一格的位置是 P141 自己搬的：它先被写在梯尾，而 `--selftest`
        // 那一趟根本排不到它）。真写一次、真被采样腿读到那一半需要一份能落的文件，它有自己的门
        // `--settings`。
        {
            let (passed, detail) = crate::settings_view::line();
            check("settings categories and write gate", passed, detail);
        }

        // §5.9.15 on the machine: the mode a click in the menu picks, and the two
        // halves the PRD says change together.
        let (alpha_ok, alpha_said) = alpha_mode_check();
        check("a background mode reaches pixels", alpha_ok, alpha_said);

        // §5.9.14 on the machine rather than in a unit test: the file door, the
        // clip, the timer's call and the pixels that actually went out.
        let (gif_ok, gif_said) = gif_clock_check();
        check("the gif clock moves frame and pixels", gif_ok, gif_said);

        // §5.9.17's legs that need no clipboard: the block this file writes is the
        // block a drop target reads, a block that is not a block is an error rather
        // than an empty drop, and a copy that promised a file is refused before it
        // promises anything.
        let (clip_ok, clip_said) = clip_file_check();
        check("copy can hand a file too", clip_ok, clip_said);

        // §5.10.1 落在真机上的一手：单击只留一枚、Ctrl+单击加、同一枚再按一次是**减**，
        // 而窗侧那一格 `view_data(id).selected` 与名单读的是同一批。点一枚不存在的贴图必须
        // 交 Err 且名单一枚也不动。用 selftest 此刻已经开着的那两枚窗，而不是另开一枚 demo：
        // 这一问要判的是**用户正在用的那两扇窗**会不会跟着名单变。
        let _ = with(|s| s.clear_selection());
        let one = with(|s| s.select_pin(id, false)).unwrap_or(0);
        let flagged = with(|s| (s.view_data(id).selected, s.view_data(board).selected));
        let who = with(|s| s.selected_ids());
        let two = with(|s| s.select_pin(board, true)).unwrap_or(0);
        let back = with(|s| s.select_pin(board, true)).unwrap_or(0);
        let ghost = with(|s| s.select_pin(4_000_003, false)).is_err();
        let still = with(|s| s.selection_count());
        let took = with(|s| s.clear_selection());
        let after = with(|s| s.selection_count());
        check(
            "multi-select is one list two readers",
            one == 1
                && flagged == (true, false)
                && who == [id]
                && two == 2
                && back == 1
                && ghost
                && still == 1
                && took == 1
                && after == 0,
            format!(
                "one={one} flagged={flagged:?} who={who:?} two={two} back={back} \
                 ghost={ghost} still={still} took={took} after={after}"
            ),
        );

        // §5.10.2 落在真机上的一手：拖一扇**被选中的**窗，名单里其余那一枚必须跟着走同样的
        // 位移；而每一枚各自被地板接住。用的还是 selftest 此刻已经开着的那两枚窗，不另开一列：
        // 「相同位移」只有两枚都活着、都在真桌面上被搬动的时候才算量过。
        //
        // 判据一枚也不并：① 名单里两枚一起走（`moved==2`、两枚都落在**自己**的落点上），且这一圈
        // 重新问一遍几何不许多画出一张纹理（两枚的 `revision` 各与搬动前相等 —— `content_key`
        // 不含位置，动了就是每一次鼠标移动都编一张 PNG）；② 名单里只留一枚时，另一枚一步也不动
        // （`is_selected` 写反就会在这一臂红）；
        // ③ 名单里那一枚**不是**被拖的那一枚时，只搬被拖的（§5.9.1 那一手不许顺手搬走别人的
        // 贴图）；④ 不在集里的一枚交 0 且什么也不搬（一扇已关的窗剩下的鼠标事件）。
        //
        // 期望落点不是猜的：每一枚的落点由 `desk.keep_visible` 从它**自己**的矩形现读——那是
        // core 那一手写的规则，抄在这里是为了让"另一枚撞了墙、这一枚跟着停下"这种坏法读得出。
        // `b1 != b0 || bw == b0` 那一枚把"桌面没给它地方"与"根本没去搬它"分开：前者是环境的
        // 答案，后者才是产品的红（#453 同一判据）。
        let _ = with(|s| s.clear_selection());
        let _ = with(|s| s.select_pin(id, false));
        let _ = with(|s| s.select_pin(board, true));
        let i0 = with(|s| (s.view_data(id).pos_x, s.view_data(id).pos_y));
        let b0 = with(|s| (s.view_data(board).pos_x, s.view_data(board).pos_y));
        // 期望落点由 core 那一手写下的规则现读，但矩形走 [`PinState::pin_rects`] 而不是
        // `view_data` 的 `size_*`：那两枚字段是给 QML 那一侧的整数，`keep_visible` 要的是
        // u32 —— 中间架一枚 `as` 的桥，就是让「这扇窗此刻占的是桌面上哪一块」由一次类型转换
        // 来猜（#297 抽不到不等于对，反过来「凑得上」也不等于问对了同一件事）。
        let want = |s: &PinState, one: PinId, dx: i32, dy: i32| -> (i32, i32) {
            let r = s
                .pin_rects()
                .into_iter()
                .find(|(p, _)| *p == one)
                .map(|(_, r)| r)
                .expect("selftest 这两枚窗从头到尾都在集里");
            let got = s
                .desk
                .keep_visible(&PhysRect::new(r.x + dx, r.y + dy, r.w, r.h));
            (got.x, got.y)
        };
        let iw = with(|s| want(s, id, 37, 21));
        let bw = with(|s| want(s, board, 37, 21));
        let rev0 = with(|s| (s.view_data(id).revision, s.view_data(board).revision));
        let moved = with(|s| s.drag_pin(id, 37, 21));
        let i1 = with(|s| (s.view_data(id).pos_x, s.view_data(id).pos_y));
        let b1 = with(|s| (s.view_data(board).pos_x, s.view_data(board).pos_y));
        // 一次纯搬动不许换走一张纹理：`content_key` 不含位置，所以批量那一圈 `sync` 每一枚都该
        // 在「key 没变」那一句上回来。这一问在这一腿上有真像素可问（两枚都是 demo 贴图），而量规
        // 那一腿只能问本窗自己那一枚。
        let rev = with(|s| (s.view_data(id).revision, s.view_data(board).revision));

        // 第二臂：名单里只剩这一枚，另一枚必须留在原地。
        let _ = with(|s| s.select_pin(id, false));
        let lw = with(|s| want(s, id, -19, -11));
        let lone = with(|s| s.drag_pin(id, -19, -11));
        let l1 = with(|s| (s.view_data(id).pos_x, s.view_data(id).pos_y));
        let lb1 = with(|s| (s.view_data(board).pos_x, s.view_data(board).pos_y));

        // 第三臂：被拖的那一枚**不在**名单里（名单里是另一枚）。
        let _ = with(|s| s.select_pin(board, false));
        let ow = with(|s| want(s, id, 13, 7));
        let off = with(|s| s.drag_pin(id, 13, 7));
        let o1 = with(|s| (s.view_data(id).pos_x, s.view_data(id).pos_y));
        let ob1 = with(|s| (s.view_data(board).pos_x, s.view_data(board).pos_y));
        let gone = with(|s| s.drag_pin(4_000_006, 5, 5));
        check(
            "one drag moves the list it was told to",
            moved == 2
                && i1 == iw
                && b1 == bw
                && (b1 != b0 || bw == b0)
                && rev == rev0
                && lone == 1
                && l1 == lw
                && lb1 == b1
                && off == 1
                && o1 == ow
                && ob1 == b1
                && gone == 0,
            format!(
                "batch={moved} id={i0:?}->{i1:?} want={iw:?} board={b0:?}->{b1:?} bwant={bw:?} \
                 rev={rev:?} 搬动前={rev0:?} lone={lone} l={l1:?} lwant={lw:?} \
                 board_stayed={lb1:?} off={off} o={o1:?} owant={ow:?} board_stayed2={ob1:?} \
                 gone={gone}"
            ),
        );

        // §5.10.3 落在真机上的一手：名单里两枚一起按一步阶梯，而每一枚按的是**自己**那一级；
        // 绝对值那一手在批量时就是把两枚设成同一个数（那是它的定义，不是阶梯的定义）；名单外的
        // 一枚一步也不动；不在集里的一枚交 0。判据一枚也不并，形状承上面 §5.10.2 那一臂。
        //
        // 起点必须两枚**不同**：都从 100 出发时，「各自一步」与「压成同一个数」交回的是同一对数，
        // 这一问就没有证人（#274 同一族）。摆开起点用的是本轮那两只手的单枚那一路，不是另设字段
        // ——字段是私有的，而本轮要判的是这只手。
        let _ = with(|s| s.clear_selection());
        let _ = with(|s| s.select_pin(id, false));
        let _ = with(|s| s.set_pin_opacity(id, 40));
        let _ = with(|s| s.select_pin(board, false));
        let _ = with(|s| s.set_pin_opacity(board, 70));
        let o0 = with(|s| (s.view_data(id).opacity, s.view_data(board).opacity));
        // 批量那一圈重读**不许**换走一张纹理：`opacity` 不进 `content_key`（[`PinState::view_data`]
        // 那一格写着「决定像素的字段里没有它」）。它动了就是每按一次「更透明」重编两张 PNG。
        let orev0 = with(|s| (s.view_data(id).revision, s.view_data(board).revision));
        let _ = with(|s| s.select_pin(id, false));
        let _ = with(|s| s.select_pin(board, true));
        let stepped = with(|s| s.step_pin_opacity(id, false));
        let o1 = with(|s| (s.view_data(id).opacity, s.view_data(board).opacity));
        let orev = with(|s| (s.view_data(id).revision, s.view_data(board).revision));

        // 第二臂：名单里只剩这一枚，另一枚必须留在原地。
        let _ = with(|s| s.select_pin(id, false));
        let olone = with(|s| s.step_pin_opacity(id, true));
        let l1 = with(|s| (s.view_data(id).opacity, s.view_data(board).opacity));

        // 第三臂：被点的那一枚**不在**名单里（名单里是另一枚）——只调被点的，不许顺手调别人。
        let _ = with(|s| s.select_pin(board, false));
        let ooff = with(|s| s.step_pin_opacity(id, false));
        let o2 = with(|s| (s.view_data(id).opacity, s.view_data(board).opacity));

        // 第四臂：绝对值那一手在批量时交回 2、且两枚落在同一个数上。
        let _ = with(|s| s.select_pin(id, false));
        let _ = with(|s| s.select_pin(board, true));
        let fixed = with(|s| s.set_pin_opacity(board, 100));
        let f1 = with(|s| (s.view_data(id).opacity, s.view_data(board).opacity));
        let gone = with(|s| s.step_pin_opacity(4_000_007, false));
        let gone_set = with(|s| s.set_pin_opacity(4_000_007, 50));
        check(
            "one opacity hand adjusts the list it was told to",
            o0 == (40, 70)
                && stepped == 2
                && o1 == (30, 60)
                && orev == orev0
                && olone == 1
                && l1 == (40, 60)
                && ooff == 1
                && o2 == (30, 60)
                && fixed == 2
                && f1 == (100, 100)
                && gone == 0
                && gone_set == 0,
            format!(
                "start={o0:?} step={stepped} after={o1:?} rev={orev:?} 阶梯前={orev0:?} \
                 lone={olone} l={l1:?} off={ooff} o={o2:?} fixed={fixed} f={f1:?} \
                 gone={gone} gone_set={gone_set}"
            ),
        );

        // §5.10.4 那三手加 §5.9.18 那两笔账落在真机上的一手 —— 形状承上面 §5.10.2 / §5.10.3 两臂，
        // 用的还是 selftest 此刻开着的那两扇窗：另开一列就把「用户正在用的那两扇窗会不会跟着名单
        // 变」换成了一问假想题。四臂各问一件事，一枚也不并：
        //
        // ①「重置」批量：两枚各自**转开**（缩放摆成 `away`、穿透走它的 setter —— 这两只是摆法，
        // 不是被判的那只手；摆在前面是因为「回到默认」在两枚本来就默认时问不出事，#274 同一族），
        // 再让名单里被按下的那一枚走「重置」：交回的必须是名单的条数，两枚的缩放与穿透必须
        // 一起回到 `[pin] default_zoom` 那一格写的数，而**那一枚中心一步也不许动**（core 那一格
        // 写着 "a reset is not a move"，P134 把「不动」读成了左上角不许动——转开缩放的那一只手
        // 本来就围着中心转，于是「重置」把窗留在 (+16,+10) 那一格；这一格现在问的是中心，且
        // `变面` 两枚钉住「面确实变过」，中心全等才不是读空气）。名单不许被这一手用掉（§5.10.4
        // 没说它是一次性的）。
        // ②「重置」单枚：名单里只有**另一枚**时，被点的这一枚走 §5.9.4 那一路 —— 只动自己、交回 1，
        // 而名单里那一枚连一级缩放也不许被抹平；不在集里的一枚交 0。
        // ③「影响数量」对三种点名给出 2 / 1 / 0 三个互不相同的数 —— 任何一只手写成常数都会在这里红。
        // ④「销毁」带走的枚数必须与 ③ 里印出的那一个数一致；一次点名空手的销毁交 0 而**不许**把这
        // 笔撤销欠账花掉（欠账还在，⑤ 才还得出两枚）。
        // ⑤ 撤销回来的是原样：两枚都回到场上（`ids()` 与带走前那一枚数对面）、`live`，而 Qt 手里那张
        // 图不是空的 —— `null=1` 是「这扇窗没有纹理」的读法，与下面那一格 closing drops the pixels
        // 用的是同一枚 shim 自陈。撤销回来的窗若是空白的，这一臂当场红。
        //
        // 期望一枚也不写死：默认缩放 `dz` 与场上枚数都是现读的，`== 2` / `== 1` / `== 0` 钉的是
        // **名单的条数**与本手交回的数（#455/#299）。「转开了没有」不交给穿透当替身，本行自己陈明：
        // `away` 是 `dz` 往**盘子内侧**挪一格（`clamp_zoom(h+1)`；800 那一档没有上一步就退到 `h-1`），
        // 所以它与 `dz` 一定不相等，而 `turnPre` 把「两枚确实不在默认那一格」读回来钉死——上一枚行
        // 把 id 的缩放留在 300（那是它自己的判据），本行不拿别人的残留当前提，每一臂都先摆再读
        // （#433：这一格没被建立时行当场红，不许悄悄绿）。穿透 1→0 仍是批量那臂自己的证人。
        let (dz, away) = with(|s| {
            let h = clamp_zoom(s.cfg.default_zoom);
            let up = clamp_zoom(h + 1);
            (h as i32, if up != h { up } else { clamp_zoom(h - 1) })
        });
        let spin = |s: &mut PinState| {
            for one in [id, board] {
                let p = s
                    .set
                    .get_mut(one)
                    .expect("selftest 这两枚窗到这一格为止都还在集里");
                p.set_zoom(away);
                let _ = p.set_click_through(true);
            }
        };
        // 两批读各要的三枚数在这里写一次（#281：两处各写一遍「这扇窗的中心在哪」就是两把尺）。
        // 中心走 [`falcon_core::geometry::PhysRect::center`]，与 `dpr_at` / `pin_rects` 同一格算术；
        // 场上没有那一枚时交回 `(-1, -1)`，而下面 `变面` 那两枚判据读到的是 (0, 0)，全等判不出、
        // 变大也判不出，那一格自己红——缺席不冒充绿（#453/#433）。
        let centre = |s: &PinState, one: PinId| {
            s.set
                .get(one)
                .map(|p| p.window_rect().center())
                .map(|r| (r.x, r.y))
                .unwrap_or((-1, -1))
        };
        let face = |s: &PinState, one: PinId| {
            let v = s.view_data(one);
            (v.size_w, v.size_h)
        };
        with(spin);
        let (t0i, t0b, c0i, c0b, k0i, k0b, g0i, g0b, tp) = with(|s| {
            (
                s.view_data(id).zoom,
                s.view_data(board).zoom,
                s.view_data(id).click_through as u32,
                s.view_data(board).click_through as u32,
                centre(s, id),
                centre(s, board),
                face(s, id),
                face(s, board),
                ((s.view_data(id).zoom != dz) as u32) * ((s.view_data(board).zoom != dz) as u32),
            )
        });
        let _ = with(|s| s.clear_selection());
        let _ = with(|s| s.select_pin(id, false));
        let _ = with(|s| s.select_pin(board, true));
        let rb = with(|s| s.reset_pin(id) as u32);
        let (z1i, z1b, c1i, c1b, k1i, k1b, g1i, g1b, rk) = with(|s| {
            (
                s.view_data(id).zoom,
                s.view_data(board).zoom,
                s.view_data(id).click_through as u32,
                s.view_data(board).click_through as u32,
                centre(s, id),
                centre(s, board),
                face(s, id),
                face(s, board),
                s.selection_count() as u32,
            )
        });
        // 「重置」不许是一场空：面没变过，全等的那两枚读的就是空气（这一枚行的对面由这两枚数供）。
        let (grew_i, grew_b) = ((g1i != g0i) as u32, (g1b != g0b) as u32);

        // ② 单枚那一臂：名单里只有**另一枚**，被点的这一枚走 §5.9.4 那一路 —— 只动自己、交回 1。
        with(spin);
        let _ = with(|s| s.select_pin(board, false));
        let (z2i, z2b, c2b) = with(|s| {
            (
                s.view_data(id).zoom,
                s.view_data(board).zoom,
                s.view_data(board).click_through as u32,
            )
        });
        let lr = with(|s| s.reset_pin(id) as u32);
        let (z3i, z3b, c3b) = with(|s| {
            (
                s.view_data(id).zoom,
                s.view_data(board).zoom,
                s.view_data(board).click_through as u32,
            )
        });
        let gr = with(|s| s.reset_pin(4_000_008) as u32);

        // ③「影响数量」：同一枚判据对三种点名，交回三个互不相同的数。
        let _ = with(|s| s.select_pin(id, false));
        let _ = with(|s| s.select_pin(board, true));
        let rcb = with(|s| s.destroy_answer(id) as u32);
        let _ = with(|s| s.select_pin(board, false));
        let rcl = with(|s| s.destroy_answer(id) as u32);
        let rcg = with(|s| s.destroy_answer(4_000_009) as u32);

        // ④⑤ 带走、空手、还回来。
        let _ = with(|s| s.select_pin(id, false));
        let _ = with(|s| s.select_pin(board, true));
        let (ib, car) = with(|s| (s.ids().len() as u32, s.destroy_pin(id) as u32));
        let (shr, owed) = with(|s| (ib - s.ids().len() as u32, s.trashed() as u32));
        let gd = with(|s| s.destroy_pin(4_000_010) as u32);
        let kept = with(|s| s.trashed() as u32);
        let und = with(|s| s.restore_pins() as u32);
        let (ia, lia, lib) = with(|s| {
            (
                s.ids().len() as u32,
                s.view_data(id).live as u32,
                s.view_data(board).live as u32,
            )
        });
        let (xia, xib) = (
            (!shim::self_check(id).contains("null=1")) as u32,
            (!shim::self_check(board).contains("null=1")) as u32,
        );
        let (owa, una) = with(|s| (s.trashed() as u32, s.restore_pins() as u32));
        let (gcl, gra) = with(|s| {
            (
                s.close_pin(4_000_011) as u32,
                s.destroy_answer(4_000_012) as u32,
            )
        });
        check(
            "one reset and one destroy answer the list they were told to",
            c0i == 1
                && c0b == 1
                && t0i == t0b
                && tp == 1
                && rb == 2
                && z1i == dz
                && z1b == dz
                && c1i == 0
                && c1b == 0
                && k1i == k0i
                && k1b == k0b
                && grew_i == 1
                && grew_b == 1
                && rk == 2
                && lr == 1
                && z2i == t0i
                && c2b == 1
                && z3i == dz
                && z3b == z2b
                && c3b == 1
                && gr == 0
                && rcb == 2
                && rcl == 1
                && rcg == 0
                && car == 2
                && shr == 2
                && owed == 2
                && gd == 0
                && kept == 2
                && und == 2
                && ia == ib
                && lia == 1
                && lib == 1
                && xia == 1
                && xib == 1
                && owa == 0
                && una == 0
                && gcl == 0
                && gra == 0,
            format!(
                "dflt={dz} turnA={t0i} turnB={t0b} turnPre={tp} ctA={c0i} ctB={c0b} \
                 rbatch={rb} homeA={z1i} homeB={z1b} ctA1={c1i} ctB1={c1b} \
                 中心A0={k0i:?} 中心A1={k1i:?} 中心B0={k0b:?} 中心B1={k1b:?} \
                 变面A={grew_i} 变面B={grew_b} \
                 roster={rk} lone={lr} loneFromA={z2i} loneFromB={z2b} \
                 loneCtB={c2b} loneHome={z3i} loneHomeB={z3b} loneCtB1={c3b} ghostReset={gr} \
                 reachBatch={rcb} reachLone={rcl} reachGhost={rcg} carried={car} \
                 onfield0={ib} shrank={shr} owed={owed} emptyhanded={gd} kept={kept} undo={und} \
                 onfield1={ia} liveA={lia} liveB={lib} pixelsA={xia} pixelsB={xib} owedAfter={owa} \
                 undoAgain={una} ghostClose={gcl} ghostReach={gra}"
            ),
        );

        // §5.11 步骤 3 那一次**全体扇出**，一扇窗看不见自己之外的窗：`pin_check.rs` 里问「其余那几枚」
        // 的那两行，证人只有被点中的那一枚（窗上的量规只能读自己的那扇窗），另几枚必须在这一格对面。
        // 四臂各问一件事，一枚也不并：
        //
        // ① 进：点名的那一枚留下，**其余**被带走。交回的数必须等于菜单在用户点下去**之前**问出的那个
        //    数（两只手共读 [`PinState::solo_kept_table`]，印着的与真带走的不许各走各的），也必须等于
        //    `场上枚数 - 留下枚数`（场上枚数是现读的，不写死）。而被点中的那枚自己一步也不许动，
        //    **没被点名的 `board`** 却必须真的变过脸 —— 压暗那一臂把它的 opacity 落到配置那一格、
        //    活性与 `solo_hidden` 都不动；隐藏那一臂把它的 `solo_hidden` 立起来、opacity 一步不动。
        //    两臂由同一枚配置键选路，所以这一格两种跑法都只跑其中一支（`soloDims=` 印出跑的是哪一支）。
        // ② 退：同一只手再按一次，放回的就是刚带走的那些枚（`xop == sop`），`board` 的 opacity 与活性
        //    和被点名那枚的 opacity 一起回到摆好那一步的数，账上不留；而**开着独显时再问菜单那一句**
        //    必须交回 0 —— 放回的那几张是 core 记的账（`Solo::saved`），桥侧一层副本也没有，硬编一个
        //    数就是拿猜测当读数。
        // ③ 空手：`ctrl+shift+s` 那一支没有「被点中的那一枚」，PRD 步骤 1 那句「用户选中一张或多张
        //    贴图」就是它的全部输入 ⇒ 名单空时交回 0、独显没开、账上 0 枚，一件也不许动。
        // ④ 幽灵：点一枚不在集里的，桥在 core 之前就该拒（[`PinState::hand_targets`] 交回空表），
        //    场上枚数与那枚真窗的活性都不许被这一手用掉（#455：0 枚不发号，也不扮成一次成功）。
        //
        // 「变过脸」不交给旁证：`base` 是本格先摆上去的，且与 `sdim` **一定不相等**（sdim≥50 取一半、
        // 否则加 50，两支都落在 0..=100 内），所以 `soloMoved=` 那一枚不是读空气；而上一枚行把两枚的
        // opacity 留在 100，本行不拿别人的残留当前提，每一臂都先摆再读（#433：判据要先被建立）。
        let (sd, sdim) = with(|s| (s.solo_dims() as u32, s.cfg.solo_dim_opacity.min(100)));
        let base = if sdim >= 50 { sdim / 2 } else { sdim + 50 };
        let _ = with(|s| s.clear_selection());
        let _ = with(|s| s.set_pin_opacity(id, base));
        let _ = with(|s| s.set_pin_opacity(board, base));
        let (pile, p0i, p0b, h0b) = with(|s| {
            (
                s.ids().len() as i64,
                s.view_data(id).opacity as i64,
                s.view_data(board).opacity as i64,
                s.view_data(board).solo_hidden as u32,
            )
        });
        let reach = with(|s| s.solo_answer(Some(id))) as i64;
        let (sop, sac, slm) = with(|s| {
            let o = s.solo_pin(Some(id));
            (o.changed as i64, o.active as u32, o.list_moved as u32)
        });
        let (p1i, p1b, l1b, h1b, l1i, k1, rch_on, sel1) = with(|s| {
            (
                s.view_data(id).opacity as i64,
                s.view_data(board).opacity as i64,
                s.view_data(board).live as u32,
                s.view_data(board).solo_hidden as u32,
                s.view_data(id).live as u32,
                s.solo_kept() as i64,
                s.solo_answer(Some(id)) as i64,
                s.view_data(id).selected as u32,
            )
        });
        let (xop, xac) = with(|s| {
            let o = s.solo_pin(Some(id));
            (o.changed as i64, o.active as u32)
        });
        let (p2i, p2b, l2b, h2b, k2) = with(|s| {
            (
                s.view_data(id).opacity as i64,
                s.view_data(board).opacity as i64,
                s.view_data(board).live as u32,
                s.view_data(board).solo_hidden as u32,
                s.solo_kept() as i64,
            )
        });
        // 空手与幽灵各按一次；这两手都该是「一件也没动」，而「没动」的证人就是上面刚读到的那些数
        // 原样还在（`l_after` / `pile_after`），本格不另开一扇窗来制造一个本来就没人动的对照。
        let _ = with(|s| s.clear_selection());
        let (nop, nac, nag, k3) = with(|s| {
            let a = s.solo_answer(None) as i64;
            let o = s.solo_pin(None);
            (a, o.changed as i64, o.active as u32, s.solo_kept() as i64)
        });
        let (gop, gac, gk, pile_after, l_after) = with(|s| {
            let o = s.solo_pin(Some(4_000_013));
            (
                o.changed as i64,
                o.active as u32,
                s.solo_kept() as i64,
                s.ids().len() as i64,
                s.view_data(id).live as u32,
            )
        });
        let dimmed = sd == 1;
        // 每一枚印出去的数都要在条件里被钉住一枚**期望**，而这一行的期望有一半由配置选路
        // （压暗那一臂与隐藏那一臂要的对面不同）。所以那些对面先按当那一臂算成变量，再让条件
        // 把实测与应有摆成 `实测 == 应有` 一对：本档的 `[row]` 那一腿读的就是这一对（#271 一枚
        // 在别的因由下也会动的读数供不出这一句；而把分支写进条件里的表达式，档面上就只剩一枚
        // 没人钉住的数）。
        let want_sd: u32 = if sdim > 0 { 1 } else { 0 };
        let want_b: i64 = if dimmed { sdim as i64 } else { p0b };
        let want_h: u32 = if dimmed { 0 } else { 1 };
        let want_l: u32 = if dimmed { 1 } else { 0 };
        let want_take: i64 = pile - 1;
        // 本格自己声明的那枚前提也印出去：`base` 与 `sdim` 一定不相等，否则「压暗那一臂压没压」
        // 在这一格里读不出差别（#433 那枚恒真的洞），所以这一枚必须是 1 而不是被假定。
        let base_gap: u32 = if base != sdim { 1 } else { 0 };
        let moved = if dimmed {
            (p1b != p0b) as u32
        } else {
            (h1b == 1 && h0b == 0) as u32
        };
        check(
            "solo puts the rest of the field away and only the rest back",
            moved == 1
                && base_gap == 1
                && sd == want_sd
                && p1b == want_b
                && h1b == want_h
                && l1b == want_l
                && p1i == p0i
                && h0b == 0
                && sop == want_take
                && l1i == 1
                && sel1 == 1
                && sac == 1
                && k1 == 1
                && reach == sop
                && rch_on == 0
                && slm == 1
                && xop == sop
                && xac == 0
                && k2 == 0
                && p2i == p0i
                && p2b == p0b
                && l2b == 1
                && h2b == 0
                && nop == 0
                && nac == 0
                && nag == 0
                && k3 == 0
                && gop == 0
                && gac == 0
                && gk == 0
                && pile_after == pile
                && l_after == 1,
            format!(
                "soloDims={sd} 该压暗={want_sd} 起点离档={base_gap} 该档={want_b} \
                 soloMoved={moved} \
                 pile={pile} 该带走={want_take} soloAnswerPre={reach} soloTake={sop} kept={k1} \
                 未点名B面1={p1b} 该藏1={want_h} 未点名藏1={h1b} 该活1={want_l} 未点名活1={l1b} \
                 点名A面1={p1i} 独显开={sac} 点名活1={l1i} 名单点名1={sel1} \
                 开着再问={rch_on} 名单挪了={slm} 退回={xop} 退回后活={xac} 账退后={k2} \
                 点名A面2={p2i} 未点名B面2={p2b} 未点名藏2={h2b} 未点名活2={l2b} \
                 soloRefuse={nop} 空手带走={nac} 空手开={nag} 空手账={k3} \
                 幽灵={gop} 幽灵开={gac} 幽灵账={gk} 场上退后={pile_after} 点名活末={l_after} \
                 摆前A={p0i} 摆前B={p0b} 摆前藏B={h0b}"
            ),
        );

        // §5.12 图片分组管理：建 / 命名规则 / 入组 / 整组显隐 / 重命名 / 排序 / 删除（退回与销毁
        // 两臂）在这台真机的状态机上**一次跑完**。solo 那一格给过 §5.11 的真机证人，这一格是给
        // §5.12 的对面：桥侧那六个 qinvokable 走的正是 core `PinSet` 的同一批手，判据把每一个印
        // 出去的数钉成 `实测 == 应有`（#202 并印不相减、#271 一枚只在别因下也动的数不作数、
        // #433 每一问的对面这一趟都得真的不同）。
        let (g_blank, g_dup, g1, g2) = with(|s| {
            let blank = s.create_group("   ").is_err() as u32; // EmptyGroupName
            let a = s.create_group("分组甲").ok().unwrap_or(0);
            let dup = s.create_group("分组甲").is_err() as u32; // DuplicateGroup
            let b = s.create_group("第二组").ok().unwrap_or(0);
            (blank, dup, a, b)
        });
        let (g_assign, row_cnt, row_shown, live_id0, live_board0) = with(|s| {
            let _ = s.select_pin(id, false);
            let _ = s.select_pin(board, true);
            let n = s.assign_selection_to_group(Some(g1)).ok().unwrap_or(0) as i64;
            let row = s.group_rows().into_iter().find(|r| r.0 == g1);
            (
                n,
                row.as_ref().map(|r| r.3 as i64).unwrap_or(-1),
                row.as_ref().map(|r| r.2 as u32).unwrap_or(9),
                s.view_data(id).live as u32,
                s.view_data(board).live as u32,
            )
        });
        let hide = with(|s| s.toggle_group_shown(g1).ok());
        let (hid_id, hid_board) =
            with(|s| (s.view_data(id).live as u32, s.view_data(board).live as u32));
        let show = with(|s| s.toggle_group_shown(g1).ok());
        let (show_id, show_board) =
            with(|s| (s.view_data(id).live as u32, s.view_data(board).live as u32));
        let (rn_ok, rn_hit, rn_dup) = with(|s| {
            let ok = s.rename_group(g1, "改名乙").is_ok() as u32;
            let hit = s.group_rows().iter().any(|r| r.0 == g1 && r.1 == "改名乙") as u32;
            let dup = s.rename_group(g1, "第二组").is_err() as u32; // 撞 g2 名
            (ok, hit, dup)
        });
        let first_before =
            with(|s| s.group_rows().iter().position(|r| r.0 == g1) == Some(0)) as u32;
        let mv_ok = with(|s| s.move_group(g2, -1).is_ok()) as u32;
        let g1_second = with(|s| s.group_rows().iter().position(|r| r.0 == g1) == Some(1)) as u32;
        let (del_ung, field_after_ung, live_after_ung) = with(|s| {
            let n = s.delete_group(g1, false).ok().unwrap_or(0) as i64;
            (n, s.ids().len() as i64, s.view_data(id).live as u32)
        });
        let del_destroy = with(|s| s.delete_group(g2, true).ok().unwrap_or(0) as i64);
        let del_nosuch = with(|s| s.delete_group(9_999_999, false).is_err()) as u32;
        let hide_said = (hide == Some(false)) as u32;
        let show_said = (show == Some(true)) as u32;
        check(
            "group hands cross the state machine",
            g_blank == 1
                && g_dup == 1
                && g_assign == 2
                && row_cnt == 2
                && row_shown == 1
                && live_id0 == 1
                && live_board0 == 1
                && hide_said == 1
                && hid_id == 0
                && hid_board == 0
                && show_said == 1
                && show_id == 1
                && show_board == 1
                && rn_ok == 1
                && rn_hit == 1
                && rn_dup == 1
                && first_before == 1
                && mv_ok == 1
                && g1_second == 1
                && del_ung == 2
                && field_after_ung == 2
                && live_after_ung == 1
                && del_destroy == 0
                && del_nosuch == 1,
            format!(
                "空名拒={g_blank} 重名拒={g_dup} 入组={g_assign} 甲组人数={row_cnt} 甲组显={row_shown} \
                 入组后活=({live_id0},{live_board0}) 隐答={hide_said} 隐后活=({hid_id},{hid_board}) \
                 显答={show_said} 显后活=({show_id},{show_board}) 改名={rn_ok} 名对上={rn_hit} \
                 改名重名拒={rn_dup} 甲先={first_before} 移答={mv_ok} 甲次位={g1_second} \
                 退组数={del_ung} 退组后场上={field_after_ung} 退组后活={live_after_ung} \
                 销毁空组数={del_destroy} 删不存在组拒={del_nosuch}"
            ),
        );

        // §5.12.2 的**点名**版本与「贴图如何处理」那两臂的**分界**：上面那一格证的是「名单里那几枚
        // 跟着走」，这一格证的是两件它证不了的事 ——
        // ① [`PinState::assign_group`] 收的是**递进来的那张名单**而不是此刻的选中名单：这里特意把
        //    两枚都选上、只点名一枚，那一枚进组而另一枚**照旧不在组里**。若这一只手只是
        //    `assign_selection_to_group` 的第二份写法，这一位断言当场翻。
        // ② 销毁与退回在界面上唯一的分界是**那一枚还在不在集里**（`--group-window` 门里 ⑨⑩ 两格
        //    读的就是这一格）：随组销毁的那枚不在、留在场上的那枚在。这里销毁的是自己新种的那一枚
        //    贴图，`id`／`board` 后面每一格还要用。
        let g_named = with(|s| s.create_group("点名对面组").ok().unwrap_or(0));
        let g_back = with(|s| s.create_group("退回对面组").ok().unwrap_or(0));
        let extra = with(|s| {
            s.add(demo_frame(48, 36), PhysPoint::new(120, 460))
                .ok()
                .unwrap_or(0)
        });
        let (named_take, named_only, named_kept) = with(|s| {
            let _ = s.select_pin(id, false);
            let _ = s.select_pin(board, true); // 名单里两枚，点名的只有一枚
            let n = s.assign_group(&[extra], Some(g_named)).ok().unwrap_or(0) as i64;
            let in_group = s.set.pins_in_group(g_named);
            (
                n,
                in_group.len() as i64,
                s.set.pins_in_group(g_named).contains(&board) as u32,
            )
        });
        let (back_take, back_rows) = with(|s| {
            let n = s.assign_group(&[id, board], Some(g_back)).ok().unwrap_or(0) as i64;
            let rows = s
                .group_rows()
                .into_iter()
                .find(|r| r.0 == g_back)
                .map(|r| r.3 as i64)
                .unwrap_or(-1);
            (n, rows)
        });
        let (ung_back, kept_id, kept_board) = with(|s| {
            let n = s.delete_group(g_back, false).ok().unwrap_or(0) as i64;
            (
                n,
                s.set.get(id).is_some() as u32,
                s.set.get(board).is_some() as u32,
            )
        });
        let (destroyed_cnt, gone_extra, still_id) = with(|s| {
            let n = s.delete_group(g_named, true).ok().unwrap_or(0) as i64;
            (
                n,
                s.set.get(extra).is_some() as u32,
                s.set.get(id).is_some() as u32,
            )
        });
        // 产品那一手（组销毁走 core `PinSet::destroy`）本来就不动桥侧那本 `frames` 账 —— 它收的是
        // 集里那一枚，纹理归 `sync`/`retire` 那一族管。这一格是自己种的那枚贴图，账留在这里会让后面
        // 每一格数到一枚已经不存在的位图，所以**仪器自己扫自己留下的尾**，不改产品的那只手。
        with(|s| {
            s.frames.remove(&extra);
            s.retire(extra);
        });
        check(
            "the named list moves and the two fates split",
            g_named > 0
                && g_back > 0
                && extra > 0
                && named_take == 1
                && named_only == 1
                && named_kept == 0
                && back_take == 2
                && back_rows == 2
                && ung_back == 2
                && kept_id == 1
                && kept_board == 1
                && destroyed_cnt == 1
                && gone_extra == 0
                && still_id == 1,
            format!(
                "点名交回={named_take}（应当 1）组内={named_only} 组里含未点名的板={named_kept}（应当 0）\
                 点名两枚交回={back_take} 组快照人数={back_rows} 退回交回={ung_back} 退回后在集里=({kept_id},{kept_board})\
                 销毁交回={destroyed_cnt} 销毁后在集里={gone_extra}（应当 0）同伴仍在={still_id}"
            ),
        );

        // §5.15「隐藏 / 显示 / 切换全部贴图」：这一格是 `toggle_pins` 那一路由的第一位真机证人，
        // 判的正是它必须独立成立的那件事 —— hide_all 让每扇窗都不 `live`、show_all 把它们放回来，
        // 而 show_all **不会**替**另一桩事**（这里用整组隐藏当对面）放开它单独藏起来的那枚。若两桩
        // 事共用同一格理由，放全局的那一刻会把本该仍被组藏着的那枚错误地放回桌面，#281 就当场落空。
        let (gpre_id, gpre_board) = with(|s| {
            let _ = s.clear_selection();
            let _ = s.select_pin(id, true); // 名单里只放 id，board 留作对照
            let g = s.create_group("全局对面组").ok().unwrap_or(0);
            let _ = s.assign_selection_to_group(Some(g));
            (s.view_data(id).live as u32, s.view_data(board).live as u32)
        });
        let (hidg_id, hidg_board) = with(|s| {
            let g = s
                .group_rows()
                .into_iter()
                .find(|r| r.1 == "全局对面组")
                .map(|r| r.0)
                .unwrap_or(0);
            let _ = s.toggle_group_shown(g);
            (s.view_data(id).live as u32, s.view_data(board).live as u32)
        });
        let (allh_id, allh_board, hid_ret) = with(|s| {
            let r = s.hide_all_pins();
            (
                s.view_data(id).live as u32,
                s.view_data(board).live as u32,
                r as u32,
            )
        });
        let (shown_id, shown_board, show_ret) = with(|s| {
            let r = s.show_all_pins();
            (
                s.view_data(id).live as u32,
                s.view_data(board).live as u32,
                r as u32,
            )
        });
        let (tg1_board, tg1_ret) = with(|s| {
            let r = s.toggle_all_pins();
            (s.view_data(board).live as u32, r as u32)
        });
        let (tg2_board, tg2_ret, tidied) = with(|s| {
            let r = s.toggle_all_pins();
            let b = s.view_data(board).live as u32;
            // 收尾：把对面组放回、再删组退组，别让这一格给后面的量规留下 stray 状态。
            let g = s
                .group_rows()
                .into_iter()
                .find(|r2| r2.1 == "全局对面组")
                .map(|r2| r2.0)
                .unwrap_or(0);
            let _ = s.toggle_group_shown(g);
            let _ = s.delete_group(g, false);
            (b, r as u32, s.view_data(id).live as u32)
        });
        check(
            "global show/hide/toggle answers apart from the group reason",
            gpre_id == 1
                && gpre_board == 1
                && hidg_id == 0
                && hidg_board == 1
                && allh_id == 0
                && allh_board == 0
                && hid_ret == 1
                && shown_id == 0
                && shown_board == 1
                && show_ret == 0
                && tg1_board == 0
                && tg1_ret == 1
                && tg2_board == 1
                && tg2_ret == 0
                && tidied == 1,
            format!(
                "入组前活=({gpre_id},{gpre_board}) 组隐后活=({hidg_id},{hidg_board}) \
                 全隐后活=({allh_id},{allh_board})隐答={hid_ret} \
                 全显后活=({shown_id},{shown_board})显答={show_ret} \
                 切换1后board活={tg1_board}答={tg1_ret} 切换2后board活={tg2_board}答={tg2_ret} \
                 收尾后id活={tidied}"
            ),
        );

        // Closing takes the pin and its texture out together.
        let closed = with(|s| s.close(&[id, board]));
        check(
            "closing drops the pixels",
            closed == 2 && shim::self_check(id).contains("null=1"),
            format!("closed={closed} {}", shim::self_check(id)),
        );

        // §5.21.3 的桥侧第一处读者：诊断那一半从活的 about 横幅与活的主目录里现读，令牌只
        // 夹在一枚 fixture 行里。三臂各答一个问题 —— 没点下头的臂（配置开关不论怎么摆都得
        // 省略）、开关跟着配置走的臂、两把闸都开的臂。敏感值在最后一臂里也不许活着出来。
        use falcon_core::feedback::{self, Draft, Payload};
        let about = crate::settings::about_cfg();
        let profile = std::env::var("USERPROFILE").unwrap_or_default();
        let account = profile.rsplit(['\\', '/']).next().unwrap_or("").to_string();
        let draft = Draft {
            description: "selftest 反馈包".to_string(),
            diagnostics: vec![
                crate::settings::startup_about_banner(),
                format!("profile={profile}"),
                "upstream Authorization: Bearer eyJhbGciOiJIUz12".to_string(),
            ],
            logs: vec!["retry token=fixturevalue".to_string()],
        };
        let version = env!("CARGO_PKG_VERSION");
        let unasked = feedback::payload(&draft, about.send_diagnostics, false, version, "windows");
        let gated = feedback::payload(&draft, about.send_diagnostics, true, version, "windows");
        let open = feedback::payload(&draft, true, true, version, "windows");
        let leaks = |p: &Payload| {
            let j = p.lines.join(" ");
            let mut v = Vec::new();
            if j.contains("eyJhbGciOiJIUz12") {
                v.push("bearer");
            }
            if j.contains("fixturevalue") {
                v.push("token");
            }
            if !account.is_empty() && j.contains(account.as_str()) {
                v.push("account");
            }
            v
        };
        let (l_no, l_g, l_open) = (leaks(&unasked), leaks(&gated), leaks(&open));
        check(
            "feedback bundle scrubs secrets",
            !unasked.attached
                && unasked.lines.len() == 3
                && unasked.lines[2].starts_with("machine=omitted")
                && gated.attached == about.send_diagnostics
                && open.attached
                && open.lines.len() == 2 + draft.diagnostics.len() + draft.logs.len()
                && open.lines[1] == "desc=selftest 反馈包"
                && open.lines.iter().any(|l| l.contains(feedback::MARK))
                && open.redactions >= 2
                && l_no.is_empty()
                && l_g.is_empty()
                && l_open.is_empty(),
            format!(
                "未确认臂={}行 省略={} 泄漏={:?} | 开关臂 attached={}==cfg={} 泄漏={:?} | \
                 双开臂={}行 藏={}枚 泄漏={:?} 主目录名={} | 主目录那一行现读={}",
                unasked.lines.len(),
                unasked.lines[2].starts_with("machine=omitted"),
                l_no,
                gated.attached,
                about.send_diagnostics,
                l_g,
                open.lines.len(),
                open.redactions,
                l_open,
                if account.is_empty() {
                    "(现读无主目录)"
                } else {
                    account.as_str()
                },
                open.lines
                    .iter()
                    .find(|l| l.contains("profile="))
                    .map(String::as_str)
                    .unwrap_or("(双开臂里没有 profile= 行)"),
            ),
        );

        // §5.21.2 第一枚「工具栏按钮应提供提示文本」在这一档里的读者：交出去的那一排句子
        // ——就是 QML 拿去按 index 分给每一枚钮的那一枚串——此刻在这只跑着的进程里现读。
        // 它判「每一枚钮拿到且只拿到它自己那一句」；那只气泡本体有没有真长出来判不了，因为
        // 本仓一枚输入注入器也没有（`SendInput` 零命中，`pin_check.rs` 那一格自己声明过），
        // 没有一只手能把指针停在钮上。接线那一半由下面那一格判。
        let names_s = crate::annotate::tool_names();
        let hints_s = crate::annotate::tool_hints();
        let names: Vec<&str> = names_s.split('|').collect();
        let hints: Vec<&str> = hints_s.split('|').collect();
        let bad: Vec<String> = hints
            .iter()
            .enumerate()
            .filter(|(i, h)| {
                names
                    .get(*i)
                    .is_none_or(|n| !h.starts_with(&format!("{n}：")))
                    || h.chars().count() <= 2
            })
            .map(|(i, h)| format!("{i}:{h}"))
            .collect();
        check(
            "toolbar hints name their own button",
            names.len() == crate::annotate::TOOLS.len()
                && hints.len() == crate::annotate::TOOLS.len()
                && bad.is_empty()
                && hints.iter().collect::<std::collections::HashSet<_>>().len() == hints.len(),
            format!(
                "TOOLS={}枚 names={}句 hints={}句 对不上名={} | 首句={} 末句={}",
                crate::annotate::TOOLS.len(),
                names.len(),
                hints.len(),
                if bad.is_empty() {
                    "无".to_string()
                } else {
                    bad.join(",")
                },
                hints.first().copied().unwrap_or("(空)"),
                hints.last().copied().unwrap_or("(空)"),
            ),
        );

        // 同一节那一排钮的**接线**那一半，仍在同一档里读。它读的是这份 QML 源码文本，而这份
        // 文本本来就在这枚出货 exe 里（`sendto_check::TOOLBAR_QML`，P154 为 `--sendto` 那一格
        // 抄进来的），所以这里复用同一份字节而不是再抄一枚 `include_str!`；剥注释用全档唯一
        // 那把尺 `hint_check::qml_code`（本档自写的那条 `//` 注释里引了一句 `property string
        // hint: ""` 的式样，不剥就会数出一枚不存在的空串 —— 引文不是活句，#313 / #253 / #268）。
        let qml = crate::hint_check::qml_code(crate::sendto_check::TOOLBAR_QML);
        let sites = qml
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.ends_with("Cell {") || t.ends_with("Swatch {")
            })
            .count();
        let handed = qml
            .lines()
            .filter(|l| l.trim_start().starts_with("hint: "))
            .count();
        let required = qml.matches("required property string hint").count();
        let attached = qml.matches("ToolTip.text:").count();
        let shown_by = qml.matches("ToolTip.visible:").count();
        let empty = qml.lines().filter(|l| l.trim() == "hint: \"\"").count();
        check(
            "every toolbar button site is handed its hint",
            sites > 0
                && sites == handed
                && required == 2
                && attached == required
                && shown_by == attached
                && empty == 0,
            format!(
                "钮的声明位置={sites} 递句子的位置={handed} required={required} ToolTip.text={attached} ToolTip.visible={shown_by} 空串={empty}"
            ),
        );

        // §5.21.2 第二枚「快捷键设置页应提供对应功能说明」。这一档不开第三份判断：名字与说明
        // 读 core 那一张表（`config::ACTION_TEXTS`），接没接线读装配用的那两张归宿表
        // （`hotkey::Work::of` / `hotkey::tray_run_of`），页上有没有把这句递出去读
        // `settings_view::Row::text` 此刻真交回的那一枚串 —— 三样都是现读，没有一句是这里重抄的。
        let texts = falcon_core::config::ACTION_TEXTS;
        let ids = falcon_core::config::ACTIONS;
        let misaligned: Vec<String> = ids
            .iter()
            .enumerate()
            .filter(|(i, id)| texts.get(*i).is_none_or(|t| t.0 != **id))
            .map(|(i, id)| format!("{i}:{id}"))
            .collect();
        // 名字或说明为空 = 页上多一处看不见的一句；含 `RECORD` = 那一枚串会把一行切成两行，
        // 「这一类几枚」那一条读数就此不再是几枚。
        let blank: Vec<String> = texts
            .iter()
            .filter(|(_, name, desc)| {
                name.is_empty()
                    || desc.is_empty()
                    || desc.contains(crate::settings_view::RECORD)
                    || name.contains(crate::settings_view::RECORD)
            })
            .map(|(id, _, _)| id.to_string())
            .collect();
        let wired = ids
            .iter()
            .copied()
            .filter(|id| crate::hotkey::Work::of(id).is_some())
            .count();
        check(
            "action table names each bindable id",
            texts.len() == ids.len() + 3 && misaligned.is_empty() && blank.is_empty(),
            format!(
                "名表={}枚 可绑={}枚 有手={wired}枚 无手={}枚 错位={} 空或会切行={} | 尾三枚={}",
                texts.len(),
                ids.len(),
                ids.len() - wired,
                if misaligned.is_empty() {
                    "无".to_string()
                } else {
                    misaligned.join(",")
                },
                if blank.is_empty() {
                    "无".to_string()
                } else {
                    blank.join(",")
                },
                texts[ids.len()..]
                    .iter()
                    .map(|t| t.0)
                    .collect::<Vec<&str>>()
                    .join(","),
            ),
        );

        // 页上那一格：类号问 `index_of`（名单里数出来的第几类），不在这里钉一个 5。
        let which = crate::settings_view::index_of("快捷键").unwrap_or(usize::MAX);
        let page = crate::settings_view::rows(which);
        let mut naming = 0usize;
        let mut said = 0usize;
        let mut quiet: Vec<String> = Vec::new();
        let mut invented: Vec<String> = Vec::new();
        let mut mis_marked: Vec<String> = Vec::new();
        for r in page.iter() {
            let t = r.text();
            match crate::settings_view::action_of(&r.key, &r.value) {
                Some((name, desc, row_wired)) => {
                    naming += 1;
                    if t.contains(name) && t.contains(desc) {
                        said += 1;
                    } else {
                        quiet.push(format!("{}={}", r.key, r.value));
                    }
                    // 标了「未接线」的行，必须正是归宿表说没有一只手的那些行。
                    if t.contains(crate::settings_view::UNWIRED_MARK) != !row_wired {
                        mis_marked.push(format!("{}={}", r.key, r.value));
                    }
                }
                None => {
                    if t.contains(crate::settings_view::NOTE_SEP) {
                        invented.push(format!("{}={}", r.key, r.value));
                    }
                }
            }
        }
        check(
            "hotkey page explains each action",
            naming > 0
                && said == naming
                && quiet.is_empty()
                && invented.is_empty()
                && mis_marked.is_empty(),
            format!(
                "快捷键那一页={}行 点名动作={naming}行 递出名与说明={said}行 没递={} 没点名却递了={} 接线标错={} | 首行={}",
                page.len(),
                if quiet.is_empty() {
                    "无".to_string()
                } else {
                    quiet.join(",")
                },
                if invented.is_empty() {
                    "无".to_string()
                } else {
                    invented.join(",")
                },
                if mis_marked.is_empty() {
                    "无".to_string()
                } else {
                    mis_marked.join(",")
                },
                page.first()
                    .map(|r| r.text())
                    .unwrap_or_else(|| "(这一类没有行)".to_string()),
            ),
        );

        // §5.21.2 第三枚 前一半：[`crate::cli::PARAM_DOC`] 那张表现在是 §5.17 语法的唯一拼写处，
        // 这一格量的就是「同源」这件事真成立 —— 方向是**把文档点名的旗标一枚枚喂回 classify 问它收不
        // 收**，而不是比两份字符串：比句子量到的是字，量不到「收下」。反方向（语法多认一枚文档没讲的
        // 旗标）在这一改之后结构上不可能发生，因为语法就是从这张表里数出来的；shipped 的 `--help` 恰
        // 恰漏讲 `--output` 与 `-h` 两枚真被认的旗标，就是旧的两本账已经对不上的读数（本轮补上）。
        let no_reserved: &[&str] = &[];
        let mut doc_flags: Vec<String> = Vec::new();
        for f in crate::cli::verbs()
            .into_iter()
            .chain(crate::cli::outputs())
            .chain(crate::cli::help_flags())
        {
            doc_flags.push(f.to_string());
        }
        let not_accepted: Vec<String> = doc_flags
            .iter()
            .filter(|f| {
                crate::cli::classify(&[(*f).clone()], no_reserved) == crate::cli::Decision::NotCli
            })
            .cloned()
            .collect();
        // 文档里 `-o file <路径>` 那一行的第三个记号是一枚占位，说的正是「这一枚值还欠一个
        // 东西」；把 `pin` 与 `file` 当成同一种东西去问「收不收」，量的是本轮探针自己的空想
        // （代次 H 的 arm A 就是这么红的：`递不进去=file`）。所以欠的那一个由
        // [`crate::cli::output_cases`] 从文档自己的示例行里取回来补齐再喂。
        let doc_cases = crate::cli::output_cases();
        let doc_targets: Vec<String> = doc_cases.iter().map(|(v, _)| v.clone()).collect();
        // 案例表把「只点名选项、不点值」的 `--output` 那一行跳过了；这一格问的就是它跳过的
        // 恰好只有那一行 —— 值名单与案例表数的是同一批行，谁也没漏。
        let listed_targets: Vec<String> = crate::cli::output_targets()
            .into_iter()
            .map(|t| t.to_string())
            .collect();
        let value_lists_agree = doc_targets == listed_targets;
        let dead_targets: Vec<String> = doc_cases
            .iter()
            .filter(|(_, argv)| {
                !matches!(
                    crate::cli::classify(argv, no_reserved),
                    crate::cli::Decision::Run { .. }
                )
            })
            .map(|(v, _)| v.clone())
            .collect();
        // 喂进去的最厚那一条，就是这一格真正问过的完整写法（不是值名，是 argv）。
        let fed_longest = doc_cases
            .iter()
            .map(|(_, argv)| argv.join(" "))
            .max_by_key(|s| s.chars().count())
            .unwrap_or_default();
        // 值的名单还得**拒得掉**：文档没讲的值，语法不许把它当成一枚输出。
        let bogus_taken = matches!(
            crate::cli::classify(
                &[
                    "--last".to_string(),
                    "-o".to_string(),
                    "dropbox".to_string()
                ],
                no_reserved,
            ),
            crate::cli::Decision::Run { .. }
        );
        check(
            "cli doc names every flag it ships",
            !doc_flags.is_empty()
                && not_accepted.is_empty()
                && !doc_targets.is_empty()
                && dead_targets.is_empty()
                && value_lists_agree
                && !bogus_taken,
            format!(
                "文档点名旗标={}枚 语法不认={} 文档的 -o 值={}枚 递不进去={} 值名单同源={} 最完整一条argv={} 文档没讲的值被收下={}",
                doc_flags.len(),
                if not_accepted.is_empty() {
                    "无".to_string()
                } else {
                    not_accepted.join(",")
                },
                doc_targets.len(),
                if dead_targets.is_empty() {
                    "无".to_string()
                } else {
                    dead_targets.join(",")
                },
                if value_lists_agree { "是" } else { "否" },
                fed_longest,
                if bogus_taken { "是" } else { "否" },
            ),
        );

        // §5.21.2 第三枚 后一半：命令行配置处真的把入口递到行上。「该不该递」问
        // [`crate::settings_view::doc_entry_of`] 那一次，句子里点的是哪一枚旗标从文档表现读，
        // 再喂回 classify 问它真能不能取到文档 —— 反推页上那一串的长相就是第二份实现。
        let which_cli = crate::settings_view::index_of("高级与命令行").unwrap_or(usize::MAX);
        let page_cli = crate::settings_view::rows(which_cli);
        let help_set = crate::cli::help_flags();
        let mut handed = 0usize;
        let mut quiet: Vec<String> = Vec::new();
        let mut invented: Vec<String> = Vec::new();
        let mut flag_named: Option<String> = None;
        for r in page_cli.iter() {
            match crate::settings_view::doc_entry_of(&r.key) {
                Some(s) => {
                    handed += 1;
                    if r.text().contains(&s) {
                        flag_named = help_set
                            .iter()
                            .find(|f| s.contains(**f))
                            .map(|f| f.to_string());
                    } else {
                        quiet.push(format!("{}={}", r.key, r.value));
                    }
                }
                None => {
                    if r.text().contains(crate::settings_view::NOTE_SEP) {
                        invented.push(format!("{}={}", r.key, r.value));
                    }
                }
            }
        }
        let flag_hands_docs = match &flag_named {
            Some(f) => {
                crate::cli::classify(std::slice::from_ref(f), no_reserved)
                    == crate::cli::Decision::Help
            }
            None => false,
        };
        check(
            "cli config row hands a doc entry",
            handed > 0 && quiet.is_empty() && invented.is_empty() && flag_hands_docs,
            format!(
                "高级与命令行那一页={}行 递入口={handed}行 该递却没递={} 没点名却递了={} 入口点的旗标={flag_named:?} 那一枚真能取到文档={}",
                page_cli.len(),
                if quiet.is_empty() { "无".to_string() } else { quiet.join(",") },
                if invented.is_empty() { "无".to_string() } else { invented.join(",") },
                if flag_hands_docs { "是" } else { "否" },
            ),
        );

        // §5.21.2 第四枚 第一格：那张表自己合不合规矩。名单里的类名要在 §5.20.1 那张表上，
        // 句子里反引号点出的每一枚键要**真在档里、且真在这一页上** —— P159 为 `--help` 立的那条
        // 规矩在这一页的翻版：引导里点一枚不存在的键，比不点更坏（用户会去那一页找它）。
        // 取 `Config::default()` 而不是本进程手里那一份：引导说的是「这一页管哪些事」，那是档的
        // 结构，不是这台机器此刻的数。
        let cfg_flat = Config::default();
        let all_keys = crate::guide::default_keys();
        let mut g_no_page: Vec<String> = Vec::new();
        let mut g_invented: Vec<String> = Vec::new();
        let mut g_off_page: Vec<String> = Vec::new();
        let mut g_blank: Vec<String> = Vec::new();
        for (gpage, gbody) in crate::guide::ENTRIES {
            let which = match crate::settings_view::index_of(gpage) {
                Some(i) => i,
                None => {
                    g_no_page.push(gpage.to_string());
                    continue;
                }
            };
            if gbody.is_empty() || gbody.contains('\n') {
                g_blank.push(gpage.to_string());
            }
            let owned: Vec<String> = crate::settings_view::rows_of(&cfg_flat, which)
                .into_iter()
                .map(|r| r.key)
                .collect();
            for token in crate::guide::dotted(gbody) {
                if !all_keys.contains(&token) {
                    g_invented.push(format!("{gpage}→{token}"));
                } else if !owned.contains(&token) {
                    g_off_page.push(format!("{gpage}→{token}"));
                }
            }
        }
        let g_complex = crate::guide::complex_pages();
        check(
            "guide names only real keys",
            g_no_page.is_empty()
                && g_invented.is_empty()
                && g_off_page.is_empty()
                && g_blank.is_empty()
                && !g_complex.is_empty()
                && g_complex.len() == crate::guide::ENTRIES.len(),
            format!(
                "复杂页={}枚 / §5.20.1 那 {} 类里；句子里点档里没有的键={} 点对了名但不在那一页={} 名单外的类名={} 空句或换行={}",
                g_complex.len(),
                crate::settings_view::CATEGORIES.len(),
                if g_invented.is_empty() { "无".to_string() } else { g_invented.join(",") },
                if g_off_page.is_empty() { "无".to_string() } else { g_off_page.join(",") },
                if g_no_page.is_empty() { "无".to_string() } else { g_no_page.join(",") },
                if g_blank.is_empty() { "无".to_string() } else { g_blank.join(",") },
            ),
        );

        // 第二格：「只递一次」这件事的三个时刻，全部在**内存里的那份状态**上问 —— 一份空的、
        // 一份收下过一枚的、另一份全新的。第三趟那一次是这一格唯一能证「一次住在状态里、不是
        // 住在进程里」的形状：只有进程内的布尔的话，换一份状态也不会再递。
        // 末尾那一枚 `attempts()` 要的是 0：默认那一趟连一次也不许动那份文件（§9.1 第十五条），
        // 而这一格印出的那句 `file_summary` 是这台机器此刻真正在读的落点。
        let g_fresh = crate::guide::State::default();
        let mut g_quiet: Vec<String> = Vec::new();
        let mut g_loud: Vec<String> = Vec::new();
        for i in 0..crate::settings_view::CATEGORIES.len() {
            let line = crate::guide::line_for(&g_fresh, i);
            if crate::guide::body(i).is_some() {
                if !line.starts_with(crate::guide::GREETING)
                    || !line.contains(crate::guide::ACK_WORD)
                {
                    g_quiet.push(crate::settings_view::CATEGORIES[i].0.to_string());
                }
            } else if !line.is_empty() {
                g_loud.push(crate::settings_view::CATEGORIES[i].0.to_string());
            }
        }
        let g_first = g_complex.first().copied().unwrap_or(usize::MAX);
        let mut g_kept = g_fresh.clone();
        let g_new_once = g_kept.mark(g_first);
        let g_new_twice = g_kept.mark(g_first);
        let g_after = crate::guide::line_for(&g_kept, g_first);
        let g_again = crate::guide::line_for(&crate::guide::State::default(), g_first);
        let g_read_back = crate::guide::State::decode(&g_kept.encode());
        let g_still_handed = (0..crate::settings_view::CATEGORIES.len())
            .filter(|i| crate::guide::body(*i).is_some() && *i != g_first)
            .all(|i| !crate::guide::line_for(&g_kept, i).is_empty());
        check(
            "first-use guide hands once",
            g_quiet.is_empty()
                && g_loud.is_empty()
                && g_new_once
                && !g_new_twice
                && g_after.is_empty()
                && !g_again.is_empty()
                && g_read_back.has(g_first)
                && g_read_back.pending() == g_complex.len().saturating_sub(1)
                && g_still_handed
                && crate::guide::attempts() == 0,
            format!(
                "一份空状态递={}枚 名单外却递={} 该递没递={}；收下{}那一页：头一回算新收={} 第二回算新收={}；收完之后这一页递={:?}、别页照递={}；换一份全新状态又递={:?}；写回来的状态读回还欠={}枚（要的是 {}）；本趟动过盘={}回（要 0）；桥现在说「{}」；{}",
                g_complex.len(),
                g_loud.len(),
                if g_quiet.is_empty() { "无".to_string() } else { g_quiet.join(",") },
                crate::settings_view::CATEGORIES.get(g_first).map(|(n, _)| *n).unwrap_or("?"),
                g_new_once,
                g_new_twice,
                g_after,
                g_still_handed,
                if g_again.is_empty() { "没递".to_string() } else { "又递了".to_string() },
                g_read_back.pending(),
                g_complex.len().saturating_sub(1),
                crate::guide::attempts(),
                crate::guide::why_empty(&g_kept, g_first),
                crate::guide::file_summary(),
            ),
        );

        let problems = with(|s| s.problems.clone());
        check(
            "no problems were noted",
            problems.is_empty(),
            problems.join(" | "),
        );

        out.push(format!("selftest: {}", if ok { "PASS" } else { "FAIL" }));
    }

    (ok, out.join("\n"))
}

/// [`clip_file_check`]'s plant verdict, as a function of the three facts it is allowed to see.
///
/// Split out because a live run can only ever produce **one** of these faces (this session either
/// opens the clipboard or it does not, and nothing in here writes it, so `Some(true)` is not
/// reachable at all), while a verdict that has never been shown to bite on the other two is a ruler
/// whose reading of the one it did get is worth nothing. `kept` is what *that* sweep reported, not
/// what the directory listing says afterwards - the two may differ when a file would not delete,
/// and the rule is about what the sweep claims to have done.
/// 三枚答案里只有一枚许可删除，所以这一枚函数写的是一句 `may_delete` 而不是三枚分支：被点名与读不开
/// 要求的命运本来就只能相同（一枚也不许动），把它们写成两枚同体的分支是 clippy 的 `match_same_arms`
/// 那一枚，而把它们合成一枚 `unwrap_or(false)` 才是真错 —— 那会把两枚理由混成一枚读数（#453）。
fn plant_fate_for(claimed: Option<bool>, alive: bool, kept: bool) -> bool {
    // 唯一许可删除的那一面：剪贴板答得出来、并且说「这枚不在我手上」。
    let may_delete = claimed == Some(false);
    if may_delete {
        // §6.2「按策略自动清理」：没被点名的那一路必须走，而且走的那一枚要 sweep 自己承认。
        !alive && !kept
    } else {
        // §5.9.17「临时文件在剪贴板失效前不得删除」：被点名 ⇒ 活着；读不开 ⇒ 也是活着（unknown 不是
        // 「没被引用」）。两枚理由相同的地方只有「不许动」这一条，所以这里不区分它们。
        alive && kept
    }
}

/// §5.9.17's answers, none of which may **write** the clipboard.
///
/// `OpenClipboard` does happen in here, and that is a different thing: [`PinState::sweep_clip_files`]
/// reads the clip to decide what may go, so a leg that grades the sweep cannot stay silent about it.
/// What stays out is every *write* —— `--selftest` runs unattended, on a locked session too (§10 第 51
/// 项 ② measured it), and a leg that overwrote the user's clipboard would take something away from
/// them to make a point this program has already made in its own output. The block is therefore read
/// back by this file's own writer and reader (`hdrop_payload` / `hdrop_paths`) - which is also the
/// only way to test the layout at all, since a mis-built `DROPFILES` does not fail on a real target,
/// it pastes as an empty drop.
///
/// The refusal answer is the state machine's: a pin that was never drawn is refused with
/// the same sentence [`Self::copy_image`] uses, and the refusal leaves the sequence and the
/// register of promised files exactly as it found them. A door that fails after taking a
/// number is a door that will name the next file after a file nobody asked for.
///
/// 那枚 plant 报三面、只判两面，而且这是设计而不是缺漏：让剪贴板点名一枚文件要写一次剪贴板，而上面
/// 那句口径不许 —— 所以「被点名 ⇒ 不许删」这一面的证人在 `--clipboard` 那一格和真机门的 `[clip]`
/// 那一枚里，不在这档。把这件事写在读者看得见的位置，才不会让这一行的 PASS 被读成它没说的意思。
fn clip_file_check() -> (bool, String) {
    use platform_windows::clip;

    let paths = vec![
        std::path::PathBuf::from("C:\\Temp\\falconshot-clip\\图片 _1.png"),
        std::path::PathBuf::from("C:/Temp/falconshot-clip/second.png"),
    ];
    let bytes = clip::hdrop_payload(&paths);
    let read_back = clip::hdrop_paths(&bytes).unwrap_or_default();
    let round = read_back == paths;

    // 反证（#274/#453）：同一把尺读一块截短的字节，要交 Err 而不是空集 —— 「这块没人引用」
    // 与「这块读不懂」在两处判据里都得是两个答案。
    let short_is_err = clip::hdrop_paths(&bytes[..bytes.len() / 2]).is_err();

    let frame = demo_frame(4, 4);
    let promised_nothing = match clip::write_image_with_files(&frame, &[]) {
        Err(e) => e.to_string().contains("没有可交的文件引用"),
        Ok(()) => false,
    };

    // §6.2 「按策略自动清理」的那半，在这一格里有读者：扫两遍，第二遍必须一枚也不删，
    // 而两遍交回来的「还在盘上的」是同一批。判据不是「删了多少」而是「第二遍不再删」——
    // 前者随机器上有没有旧文件变，后者只随本函数的算法变。
    let (swept, kept_first, kept_again) = with(|s| {
        let (deleted, kept) = s.sweep_clip_files();
        let (again_deleted, again_kept) = s.sweep_clip_files();
        (deleted, kept, (again_deleted, again_kept))
    });
    let sweep_stable = kept_again.0 == 0
        && kept_again.1.len() == kept_first.len()
        && kept_first.iter().all(|p| kept_again.1.contains(p));

    let (refused, kept_seq, kept_register) = with(|s| {
        let (seq_before, pending_before) = (s.clip_seq, s.clip_pending_files.len());
        let said = s.copy_image_with_files(u64::MAX).err().unwrap_or_default();
        (
            said.contains("还没有画过"),
            s.clip_seq == seq_before,
            s.clip_pending_files.len() == pending_before,
        )
    });

    // §5.9.17「临时文件在剪贴板失效前不得删除」与 §6.2「按策略自动清理」是**同一枚文件的两种命运**，
    // 所以这一腿只写一枚文件进 clip_dir，把它交给产品自己的那一趟 sweep（[`PinState::sweep_clip_files`]），
    // 然后按剪贴板此刻的答案读它该落在哪一种里：
    //
    // - 读不开（锁屏 —— 本函数跑的就是这一枚会话）⇒ 必须活着：unknown 不许删任何东西；
    // - 读得开且没点名它 ⇒ 必须被删：这一枚才是「按策略清理」那句的正面证据，而且它可推翻 —— 上面那
    //   两趟的 `sweep_stable` 说的是「第二趟不再删」，那是同一枚算法的自洽，不是「剪贴板上那一路文件
    //   不会被删」。
    //
    // 第三面（读得开**且**点名 ⇒ 活着）这一枚函数答不了，而且是故意的：让剪贴板点名一枚文件要写一次
    // 剪贴板，而 `--selftest` 的口径就是不碰用户那块剪贴板 —— 本档输出末尾那句
    // `clipboard selftest: SKIPPED (pass --clipboard to run it; it overwrites the clipboard)`
    // 立的正是同一枚牌。那一面由真机门的 `[clip]` 那枚读：那一趟跑完之后活剪贴板确实点名的就是开着那
    // 一侧的产物文件，而 `[on]` 那一行同时报它此刻在盘上。
    //
    // 本腿也不替 sweep 收尾：被判「不许删」而活着的那一枚留在盘上，正是那句禁止的形状；强行删掉它就是
    // 违反一次被测的规则来换一份干净的文件列表。下一趟读得到剪贴板的 sweep 会把它清走。
    let plant = {
        let mut p = PinState::clip_dir();
        p.push("p130-sweep-plant.png");
        p
    };
    let _ = std::fs::create_dir_all(PinState::clip_dir());
    let plant_written = std::fs::write(&plant, b"plant").is_ok();
    // 三枚答案，不是两枚：`read_files()` 交 Err 与交 `Ok(vec![])` 在这一枚判据里是两种命运
    // （前者一枚也不许删，后者正是该删的那一面），所以这里合成 `Option` 而不是
    // `unwrap_or(false)` —— 那一枚写法会把「读不开」上报成「没点名」（#453）。
    let claimed = clip::read_files().map(|files| files.contains(&plant)).ok();
    let (plant_deleted, plant_kept) = with(|s| s.sweep_clip_files());
    let plant_alive = plant.exists();
    let plant_fate = plant_fate_for(claimed, plant_alive, plant_kept.contains(&plant));
    let plant_claimed = match claimed {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    };
    let plant_sweep = format!("{plant_deleted}/{}", plant_kept.len());

    (
        round
            && short_is_err
            && promised_nothing
            && sweep_stable
            && refused
            && kept_seq
            && kept_register
            && plant_written
            && plant_fate,
        format!(
            "roundtrip={} 张; short_is_err={short_is_err}; empty_refused={promised_nothing}; \
             plant_written={plant_written}; plant_claimed={plant_claimed}; \
             plant_alive={plant_alive}; plant_sweep={plant_sweep}; plant_fate={plant_fate}; \
             sweep_deleted={swept}; sweep_kept={}; second_sweep_deleted={}; \
             second_sweep_kept={}; sweep_stable={sweep_stable}; \
             undrawn_refused={refused}; seq_kept={kept_seq}; register_kept={kept_register}",
            read_back.len(),
            kept_first.len(),
            kept_again.0,
            kept_again.1.len()
        ),
    )
}

/// The clip the two doors below build, as the frames it carries: one entry per frame,
/// `(shade, 该帧要求的 ms)`.
///
/// The frame count is here rather than only inside the loop because the gauge has to
/// *expect* a number it did not choose: `pin_check` grades the on-screen clip against
/// `GIF_TEST_FRAMES.len()`, and a fixture whose length lives in one file while its
/// expectation lives in another is a pair that drifts - a 3-frame GIF would keep passing
/// a row that asks for 2.
///
/// Black and white because GIF is palettised and those two survive the trip exactly, so
/// a test may compare a pixel rather than an approximation of one. 120 ms and 60 ms
/// because the cycle they make (180 ms) is shorter than the gauge's one wait (400 ms) by
/// a margin, so a leg that watches the frame index has a claim that does not depend on
/// which side of a boundary the sample lands on.
pub(crate) const GIF_TEST_FRAMES: [(u8, u32); 2] = [(0, 120), (255, 60)];

/// The size of those frames. Not 4x3: the `--pin` gauge pins this file and then asks the
/// operating system about the window that comes out the other side, and a window that
/// small is where a shell's own minimum size would answer instead of the product's rect.
const GIF_TEST_W: u32 = 40;
const GIF_TEST_H: u32 = 30;

/// Two frames, [`GIF_TEST_W`]x[`GIF_TEST_H`] - black, then white, asking for 120 ms and
/// 60 ms.
pub(crate) fn write_two_frame_gif(path: &std::path::Path) -> Result<(), String> {
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame as AnimFrame, ImageBuffer, Rgba};
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut enc = GifEncoder::new(&mut out);
        for (shade, pace) in GIF_TEST_FRAMES {
            let buf =
                ImageBuffer::from_pixel(GIF_TEST_W, GIF_TEST_H, Rgba([shade, shade, shade, 255]));
            enc.encode_frame(AnimFrame::from_parts(
                buf,
                0,
                0,
                Delay::from_numer_denom_ms(pace, 1),
            ))
            .map_err(|e| e.to_string())?;
        }
    }
    std::fs::write(path, out.into_inner()).map_err(|e| e.to_string())
}

/// §5.9.15 end to end: the row the menu picks, the mode the pin answers in, the
/// texture that went out with it, and which points of the window take a click.
///
/// The pixel is the answer for the first half and [`PinState::hit_at`] the answer
/// for the second, because the section's promise is that one choice moves *both* —
/// a mode that repaints but still refuses the click, or one that opens the window
/// to clicks while the board never appears, each passes a one-sided check.
fn alpha_mode_check() -> (bool, String) {
    // A hole at (0,0) and one opaque pixel at (3,1), so the two questions the
    // section asks have different answers inside the same pin.
    let mut holed = transparent_frame(8, 4);
    holed.set(3, 1, [0xff, 0x00, 0x00, 0xff]);
    let id = match with(|s| s.add(holed, PhysPoint::new(60, 60))) {
        Ok(id) => id,
        Err(e) => return (false, format!("the holed pin would not open: {e}")),
    };
    let solid = match with(|s| {
        s.add(
            Frame::filled(8, 4, [7, 7, 7, 0xff]).expect("frame"),
            PhysPoint::new(90, 60),
        )
    }) {
        Ok(id) => id,
        Err(e) => return (false, format!("the solid pin would not open: {e}")),
    };

    let v0 = with(|s| s.view_data(id));
    // The window's own top-left, in the desktop space `hit_at` is asked about. At
    // the default zoom the two spaces are one, and `same_size` is where a config
    // that changed that would say so rather than mis-answering the clicks.
    let win = PhysPoint::new(v0.pos_x, v0.pos_y);
    let same_size = v0.size_w == 8 && v0.size_h == 4;
    let hole = win;
    let ink = PhysPoint::new(win.x + 3, win.y + 1);

    let hit_hole_before = with(|s| s.hit_at(id, hole));
    let hit_ink_before = with(|s| s.hit_at(id, ink));

    // 菜单第三行 = 深色棋盘格: the hole gets a board and takes a click.
    let dark = with(|s| s.set_alpha_bg(id, 2));
    let v2 = with(|s| s.view_data(id));
    let board = shim::pixel(id, 0, 0);
    let hit_hole_dark = with(|s| s.hit_at(id, hole));

    // 菜单第二行 = 伪透明: the hole is a hole again, the click is kept.
    let pseudo = with(|s| s.set_alpha_bg(id, 1));
    let v1 = with(|s| s.view_data(id));
    let open = shim::pixel(id, 0, 0);
    let hit_hole_pseudo = with(|s| s.hit_at(id, hole));

    // 菜单第一行 = 透明: back where the pin started, click refused again.
    let clear = with(|s| s.set_alpha_bg(id, 0));
    let v_back = with(|s| s.view_data(id));
    let hit_hole_last = with(|s| s.hit_at(id, hole));
    let back = shim::pixel(id, 0, 0);

    let solid_view = with(|s| s.view_data(solid));
    let closed = with(|s| s.close(&[id, solid]));

    let passed = same_size
        && v0.alpha_bg == 0
        && v0.has_transparent_pixels
        && !solid_view.has_transparent_pixels
        && !hit_hole_before
        && hit_ink_before
        && dark
        && v2.alpha_bg == 2
        && v2.has_transparent_pixels
        && v2.revision > v0.revision
        && board == "42,42,42,255"
        && hit_hole_dark
        && pseudo
        && v1.alpha_bg == 1
        && open == "0,0,0,0"
        && hit_hole_pseudo
        && clear
        && v_back.alpha_bg == 0
        && back == "0,0,0,0"
        && !hit_hole_last
        && closed == 2;
    (
        passed,
        format!(
            "win={}x{} mode={}>{}>{}>{} pix={}|{}|{} hole={}|{}|{}|{} ink={} solid_holes={}",
            v0.size_w,
            v0.size_h,
            v0.alpha_bg,
            v2.alpha_bg,
            v1.alpha_bg,
            v_back.alpha_bg,
            board,
            open,
            back,
            hit_hole_before,
            hit_hole_dark,
            hit_hole_pseudo,
            hit_hole_last,
            hit_ink_before,
            solid_view.has_transparent_pixels,
        ),
    )
}

/// §5.9.14 end to end: a two-frame GIF on disk, the door a dropped file goes
/// through, the call the window's timer makes, and the pixels that came out the
/// other side.
///
/// The answer is the pixel a texture carries, not the frame index. An index can
/// be bumped while the encoder still ships frame zero, and that is exactly the
/// failure a user would see - a clip that plays but never changes.
fn gif_clock_check() -> (bool, String) {
    let path = std::env::temp_dir().join("falconshot-gifcheck.gif");
    if let Err(e) = write_two_frame_gif(&path) {
        return (false, format!("could not write the clip: {e}"));
    }
    let name = path.display().to_string();
    let id = match with(|s| s.add_from_file(&name, PhysPoint::new(40, 40))) {
        Ok(id) => id,
        Err(e) => return (false, format!("the clip would not pin: {e}")),
    };
    let ink = |s: &PinState| s.rendered.get(&id).map(|f| f.pixels[0]).unwrap_or(250);
    let v0 = with(|s| s.view_data(id));
    let before = with(|s| ink(s));
    // 130 ms is past the first frame's own 120 ms and not past two of them.
    let moved = with(|s| s.tick_gif(id, 130));
    let v1 = with(|s| s.view_data(id));
    let after = with(|s| ink(s));
    // Paused, ten seconds is not ten seconds of frames it owes.
    let _ = with(|s| s.edit(id, |p, _, _| p.gif.as_mut().map(|play| play.toggle_play())));
    let resumed = with(|s| s.tick_gif(id, 10_000));
    let paused_on = with(|s| ink(s));
    let closed = with(|s| s.close(&[id]));
    let clip_gone = with(|s| !s.clips.contains_key(&id));
    let _ = std::fs::remove_file(&path);
    let passed = v0.gif_frames == GIF_TEST_FRAMES.len() as i32
        && v0.gif_frame == 0
        && v0.gif_playing
        && v0.gif_speed == 100
        && moved
        && v1.gif_frame == 1
        && v1.revision > v0.revision
        && before != after
        && !resumed
        && paused_on == after
        && closed == 1
        && clip_gone;
    (
        passed,
        format!(
            "frames={} speed={} ink {}->{} then paused {} ; tick={} paused_tick={} rev={}..{} closed={} clip_gone={}",
            v0.gif_frames,
            v0.gif_speed,
            before,
            after,
            paused_on,
            moved,
            resumed,
            v0.revision,
            v1.revision,
            closed,
            clip_gone
        ),
    )
}

/// The output chain, for real: a pin onto the clipboard, the clipboard back in,
/// and a pin out to a file and read again.
///
/// Three answers, not two: another app may be holding the clipboard, and that is
/// reported as `BLOCKED` with the holder named rather than as a row of failures.
///
/// This is kept apart from [`selftest`] because it *takes the user's clipboard
/// over*. What was on it is read first and put back at the end, which is the best
/// that can be promised: a clipboard can carry formats this app does not model,
/// and those would be lost. `--selftest --clipboard`.
/// A row of the clipboard check has three answers, and they are not two.
///
/// Ordering matters as much as the names: `Fail` dominates `Blocked`, so a run
/// that is both cannot report only the excuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check {
    Pass,
    Blocked,
    Fail,
}

impl Check {
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Check::Pass => "PASS",
            Check::Blocked => "SKIP",
            Check::Fail => "FAIL",
        }
    }
}

impl From<bool> for Check {
    fn from(passed: bool) -> Self {
        if passed {
            Check::Pass
        } else {
            Check::Fail
        }
    }
}

/// Put back what the user had, as far as this app can represent it, and say how far
/// that was.
///
/// Two gauges owe the machine this now: `--selftest --clipboard` writes a test picture,
/// and `--ink`'s 复制 leg goes through the product's own clipboard call, which writes
/// the screenshot. Both take the user's contents away in order to measure something,
/// and the debt is paid here rather than in two copies of the same four branches -
/// the copy that forgets the `html or a file list` case is the one that silently eats
/// what was in front of the user, and the copy that forgets *empty* leaves its own test
/// picture on a clipboard the user had cleared.
pub fn put_back(before: &Option<platform_windows::clip::Contents>) -> Result<String, String> {
    match before {
        // `read()` answering `None` is not an empty clipboard: it is a clipboard that
        // would not open, and whatever was on it went with the write that followed. The
        // leg that reports that as "it was empty" is a leg claiming to have restored
        // what it never read.
        None => Err("it would not open for reading, so what was on it is not knowable".to_string()),
        Some(c) if c.is_empty() => platform_windows::clip::clear()
            .map(|_| "it was empty, and is empty again".to_string())
            .map_err(|e| e.to_string()),
        Some(c) => {
            if let Some(bytes) = &c.image_bytes {
                match encode::decode(bytes) {
                    Ok(frame) => platform_windows::clip::write_image(&frame)
                        .map(|_| format!("image {}x{}", frame.width, frame.height))
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                }
            } else if let Some(t) = &c.text {
                platform_windows::clip::write_text(t)
                    .map(|_| format!("text ({} chars)", t.chars().count()))
                    .map_err(|e| e.to_string())
            } else {
                Ok("html or a file list, which this check cannot put back".to_string())
            }
        }
    }
}

pub fn clipboard_selftest() -> (Check, String) {
    // Ask first, because the clipboard can belong to somebody else. When it does,
    // the honest answer is who holds it - not a row of failures that reads as if
    // this program's clipboard code were broken.
    let blocked = platform_windows::clip::usable()
        .err()
        .map(|e| e.to_string());
    let before = platform_windows::clip::read().ok();
    let mut out = Vec::new();
    let mut worst = if blocked.is_some() {
        Check::Blocked
    } else {
        Check::Pass
    };
    let mut check = |label: &str, verdict: Check, detail: String| {
        if verdict > worst {
            worst = verdict;
        }
        out.push(format!("{label:<34} {:<4} {detail}", verdict.tag()));
    };

    // Semi-transparent on purpose: whether alpha survives the trip is the whole
    // question this check exists to answer, and an opaque test picture would
    // pass by accident.
    let src = match Frame::from_rgba(
        4,
        2,
        vec![
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 64, 255, 255, 0, 255, //
            0, 255, 255, 0, 255, 0, 255, 32, 12, 34, 56, 78, 200, 100, 50, 255,
        ],
    ) {
        Ok(f) => f,
        Err(e) => return (Check::Fail, format!("test frame is not a frame: {e}")),
    };
    let id = match with(|s| s.add(src.clone(), PhysPoint::new(30, 30))) {
        Ok(id) => id,
        Err(e) => return (Check::Fail, format!("could not add a pin: {e}")),
    };

    let copied = with(|s| s.copy_image(id));
    match &blocked {
        Some(who) => check(
            "a pin copied itself",
            Check::Blocked,
            format!("clipboard held by {who}"),
        ),
        None => check(
            "a pin copied itself",
            Check::from(copied.is_ok()),
            copied
                .clone()
                .map(|n| format!("{n} pixels"))
                .unwrap_or_else(|e| e),
        ),
    }

    let formats = platform_windows::clip::formats();
    match (&blocked, &formats) {
        (Some(who), _) => check(
            "the clipboard carries a DIB",
            Check::Blocked,
            who.to_string(),
        ),
        (None, Err(e)) => check("the clipboard carries a DIB", Check::Fail, e.to_string()),
        (None, Ok(list)) => check(
            "the clipboard carries a DIB",
            Check::from(list.iter().any(|f| f == "#8")),
            list.join(","),
        ),
    }

    // Read it back through the same door §5.8.2 uses.
    let back = match platform_windows::clip::read() {
        Ok(contents) => {
            let facts = contents.facts();
            falcon_core::clip::classify(&facts)
                .map(|(_kind, payload)| match payload {
                    falcon_core::clip::ClipPayload::Image(f) => Some(f),
                    other => {
                        eprintln!("[clipboard] classified as {other:?}");
                        None
                    }
                })
                .map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    };
    match &blocked {
        Some(who) => check(
            "paste-back is the same pixels",
            Check::Blocked,
            who.to_string(),
        ),
        None => {
            let round = matches!(&back, Ok(Some(frame)) if *frame == src);
            check(
                "paste-back is the same pixels",
                Check::from(round),
                match back {
                    Ok(Some(frame)) => {
                        format!("{}x{} {:?}", frame.width, frame.height, frame.pixels)
                    }
                    Ok(None) => "not an image payload".to_string(),
                    Err(e) => e,
                },
            );
        }
    }

    let file = std::env::temp_dir().join("falconshot-clipcheck.png");
    let saved = with(|s| s.save_image(id, &file.to_string_lossy(), ""));
    let again = saved
        .as_ref()
        .ok()
        .and_then(|_| encode::decode_file(&file).ok());
    check(
        "a pin saved and re-read",
        Check::from(again.as_ref() == Some(&src)),
        format!(
            "{} -> {:?}",
            saved.unwrap_or_default(),
            again.map(|f| f.pixels).unwrap_or_default()
        ),
    );

    let closed = with(|s| s.close(&[id]));
    check(
        "the check cleaned up after itself",
        Check::from(closed == 1 && file.exists()),
        format!("closed={closed} file kept for inspection"),
    );
    let _ = std::fs::remove_file(&file);

    // Whatever the user had on the clipboard goes back, as far as this app can
    // represent it - and the check says which far that was.
    match &blocked {
        Some(who) => check("the clipboard came back", Check::Blocked, who.to_string()),
        None => {
            let restored = put_back(&before);
            check(
                "the clipboard came back",
                Check::from(restored.is_ok()),
                restored.unwrap_or_else(|e| e),
            );
        }
    }

    out.push(format!(
        "clipboard selftest: {}",
        match worst {
            Check::Pass => "PASS",
            Check::Blocked => "BLOCKED",
            Check::Fail => "FAIL",
        }
    ));
    (worst, out.join("\n"))
}

/// The capture layer checked against the real desktop: enumerate, freeze, crop,
/// pick a colour, look at a window, pin a region. `--selftest --capture`.
///
/// Every row asserts a *relationship* rather than a fixed value, because what
/// happens to be on this screen is not knowable in advance - and a relationship
/// that holds for any content (a frame exactly the size of the monitor it came
/// from) is precisely what a stretched or mis-cropped capture breaks.
///
/// `BLOCKED` is for the machine refusing to be looked at - a capture backend that
/// will not start is an environment fact, not a failing test.
pub fn capture_selftest() -> (Check, String) {
    let mut out = Vec::new();
    let mut worst = Check::Pass;
    let mut check = |label: &str, verdict: Check, detail: String| {
        if verdict > worst {
            worst = verdict;
        }
        out.push(format!("{label:<34} {:<4} {detail}", verdict.tag()));
    };

    let listed = capture::service().monitors();
    match &listed {
        Err(e) => check("monitor enumeration", Check::Blocked, e.to_string()),
        Ok(list) => {
            let primaries = list.iter().filter(|m| m.primary).count();
            let line = list
                .iter()
                .map(|m| {
                    format!(
                        "{} {}x{}@{},{} x{:.2}{}",
                        m.id,
                        m.bounds.w,
                        m.bounds.h,
                        m.bounds.x,
                        m.bounds.y,
                        m.scale.ratio(),
                        if m.primary { "*" } else { "" }
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ");
            // Two primaries is the enumeration lying about the desktop, and it
            // decides which scale a DIP readout uses (PRD §5.3.7).
            check(
                "monitor enumeration",
                Check::from(!list.is_empty() && primaries == 1),
                format!("{} monitor(s), {primaries} primary: {line}", list.len()),
            );
        }
    }

    let frozen = capture::freeze();
    match &frozen {
        Err(e) => check(
            "freeze the whole screen",
            Check::Blocked,
            format!("{e} - every row below needs a frame"),
        ),
        Ok(f) => {
            // The one shape a later crop depends on: `Frame::crop` works in the
            // frame's own pixels, so a frame smaller than its monitor turns "the
            // right-hand half" into a stretched copy instead of a crop (PRD §4.3).
            let bad: Vec<String> = f
                .snap
                .monitors
                .iter()
                .filter(|m| m.frame.width != m.info.bounds.w || m.frame.height != m.info.bounds.h)
                .map(|m| {
                    format!(
                        "{} frame {}x{} vs bounds {}x{}",
                        m.info.id, m.frame.width, m.frame.height, m.info.bounds.w, m.info.bounds.h
                    )
                })
                .collect();
            check(
                "freeze the whole screen",
                Check::from(bad.is_empty()),
                if bad.is_empty() {
                    format!("{} ms, {}", f.ms, capture::snapshot_line(&f.snap))
                } else {
                    bad.join(" | ")
                },
            );
            // P1/P5 measured 142-155 ms for one 3072x1920 BitBlt; the 150 ms line is
            // for hot key to *mask visible*, so a freeze alone gets the benefit of
            // the doubt up to a ceiling that still means something is wrong.
            check(
                "freeze is not pathological",
                Check::from(f.ms <= 400),
                format!("{} ms", f.ms),
            );
        }
    }
    let Some(snap) = frozen.as_ref().ok().map(|f| f.snap.clone()) else {
        out.push(format!(
            "capture selftest: {}",
            match worst {
                Check::Pass => "PASS",
                Check::Blocked => "BLOCKED",
                Check::Fail => "FAIL",
            }
        ));
        return (worst, out.join("\n"));
    };

    // Indexing is safe here and nowhere else: `freeze` answers `Empty` rather than
    // an snapshot with no monitors, and `snap` came from a successful freeze.
    let primary = snap
        .monitors
        .iter()
        .find(|m| m.info.primary)
        .unwrap_or(&snap.monitors[0]);
    let pb = primary.info.bounds;
    let centre = PhysPoint::new(pb.x + pb.w as i32 / 2, pb.y + pb.h as i32 / 2);

    // Win32 answers in pixels, Qt answers in device-independent pixels. Both are
    // right, and a pin's position is handed to Qt as physical - so the two have to
    // be the same desktop divided by the scale factor, or one of them is describing
    // a screen that is not there.
    {
        let (qx, qy, qw, qh) = shim::primary_bounds();
        let s = primary.info.scale;
        let close = (pb.w as f64 / s.ratio() - qw as f64).abs() <= 2.0
            && (pb.h as f64 / s.ratio() - qh as f64).abs() <= 2.0
            && s.phys_to_dip_i(pb.x) == qx
            && s.phys_to_dip_i(pb.y) == qy;
        check(
            "Win32 and Qt agree on geometry",
            Check::from(close),
            format!(
                "win32={}x{}@{},{} scale={:.2} -> dip {}x{}@{},{} qt={qw}x{qh}@{qx},{qy}",
                pb.w,
                pb.h,
                pb.x,
                pb.y,
                s.ratio(),
                s.phys_to_dip_i(pb.w as i32),
                s.phys_to_dip_i(pb.h as i32),
                s.phys_to_dip_i(pb.x),
                s.phys_to_dip_i(pb.y),
            ),
        );
    }

    // The colour picker and the magnifier both read the frozen frame, which is the
    // only reason they are correct under a dimmed mask (PRD §5.4.2).
    {
        let picked = snap.color_at(centre);
        let direct = primary
            .frame
            .get((centre.x - pb.x) as u32, (centre.y - pb.y) as u32);
        check(
            "colour pick reads the frame",
            Check::from(picked == Some(direct)),
            format!("{picked:?} vs {direct:?}"),
        );
    }

    {
        let mag = snap.magnifier(centre, 8, 4);
        check(
            "magnifier is a hard-edged zoom",
            Check::from(
                mag.as_ref()
                    .is_ok_and(|f| f.width == (8 * 2 + 1) * 4 && f.height == (8 * 2 + 1) * 4),
            ),
            match &mag {
                Ok(f) => format!("{}x{} for radius 8 zoom 4", f.width, f.height),
                Err(e) => e.to_string(),
            },
        );
    }

    // Asking for more desktop than exists must come back with what exists, and say
    // so (PRD §5.2.5/§8.5) - never an error and never a padded edge.
    {
        let want = PhysRect::new(pb.x + pb.w as i32 - 60, pb.y, 200, 80);
        let got = snap.capture(&want);
        let right = snap.virtual_bounds.x + snap.virtual_bounds.w as i32;
        check(
            "a rect off the edge is clipped",
            Check::from(
                got.as_ref()
                    .is_ok_and(|c| c.clipped && c.rect.x + c.rect.w as i32 <= right),
            ),
            match &got {
                Ok(c) => format!(
                    "asked 200 wide, got {}x{} at {},{} clipped={}",
                    c.rect.w, c.rect.h, c.rect.x, c.rect.y, c.clipped
                ),
                Err(e) => e.to_string(),
            },
        );
    }

    // Two monitors side by side: one image, no stretch, both named (PRD §5.3.10).
    {
        let pair = snap.monitors.iter().find_map(|a| {
            snap.monitors
                .iter()
                .find(|b| {
                    b.info.id != a.info.id
                        && b.info.bounds.x == a.info.bounds.x + a.info.bounds.w as i32
                })
                .map(|b| (a, b))
        });
        match pair {
            None => check(
                "a straddle stitches two frames",
                Check::Blocked,
                "no two monitors are adjacent".to_string(),
            ),
            Some((a, b)) => {
                let seam = a.info.bounds.x + a.info.bounds.w as i32;
                let got = snap.capture(&PhysRect::new(seam - 20, a.info.bounds.y + 10, 40, 20));
                check(
                    "a straddle stitches two frames",
                    Check::from(
                        got.as_ref()
                            .is_ok_and(|c| c.monitors.len() == 2 && c.frame.width == 40),
                    ),
                    match &got {
                        Ok(c) => format!(
                            "{}x{} from [{}]",
                            c.frame.width,
                            c.frame.height,
                            c.monitors.join(",")
                        ),
                        Err(e) => e.to_string(),
                    },
                );
                let _ = b;
            }
        }
    }

    {
        let wins = capture::service().windows(false);
        match wins {
            Err(e) => check("window enumeration", Check::Blocked, e.to_string()),
            Ok(list) => {
                let dwmed = list.iter().filter(|w| w.dwm_bounds.is_some()).count();
                // A window shot is cropped by the DWM rect, not by `GetWindowRect`,
                // which carries the invisible resize border along with it. Zero here
                // means the platform layer is answering nothing.
                check(
                    "window enumeration",
                    Check::from(!list.is_empty() && dwmed > 0),
                    format!(
                        "{} window(s), {dwmed} with DWM bounds; top {:?}",
                        list.len(),
                        list.first().map(|w| (w.title.clone(), w.visible_bounds())),
                    ),
                );
            }
        }
    }

    // End to end, on pixels that exist: a region of the frozen screen becomes a pin
    // of exactly that size, and the pin leaves no residue behind.
    {
        let want = PhysRect::new(pb.x + 40, pb.y + 40, 48, 32);
        let got = capture::pin_rect(&want);
        let seen = got
            .as_ref()
            .ok()
            .and_then(|(id, _)| with(|s| s.with_rendered(*id, |f| (f.width, f.height))))
            .unwrap_or((0, 0));
        let closed = match &got {
            Ok((id, _)) => with(|s| s.close(&[*id])),
            Err(_) => 0,
        };
        check(
            "a frozen region becomes a pin",
            Check::from(got.is_ok() && seen == (48, 32) && closed == 1),
            format!(
                "{} -> {}x{} closed={}",
                match &got {
                    // P165: the sentence `pin_rect` hands back is now printed here rather than
                    // dropped. A whole region's sentence is just 「区域 {rect}」, so this row reading
                    // it is the control for the clipped row below: the presenter says nothing more
                    // when nothing was cut.
                    Ok((id, line)) => format!("pin {id}: {line}"),
                    Err(e) => e.clone(),
                },
                seen.0,
                seen.1,
                closed
            ),
        );
    }

    // §5.3.5 校验规则's second bullet has two halves -- 提示 and 自动修正 -- and `fn main` is the only
    // place the first one used to be printed, which is a place `cargo test` cannot reach. P165 asks
    // the same question headless: `pin_rect` builds the picture, this row reads the line off the
    // tuple it hands back, and `close` takes the pin away before any window exists. So the sentence
    // a real machine produced is in this report, and the remembered region is the proof that the
    // clipping is what the product kept rather than what was typed.
    {
        let want = PhysRect::new(pb.x + pb.w as i32 - 24, pb.y + 40, 40_000, 32);
        match capture::pin_rect(&want) {
            Ok((id, line)) => {
                let kept = with(|s| s.captured_of(id));
                let closed = with(|s| s.close(&[id]));
                let says_both = line.contains("你要的是") && line.contains("已裁掉");
                let narrowed = kept.is_some_and(|got| got != want && got.w < want.w);
                check(
                    "an oversized region is clipped and says which number moved",
                    Check::from(says_both && narrowed && closed == 1),
                    format!(
                        "要 {} -> 记 {:?}｜说法「{}」closed={}",
                        want, kept, line, closed
                    ),
                );
            }
            Err(e) => check(
                "an oversized region is clipped and says which number moved",
                Check::Fail,
                format!("区域 {want:?} 钉不起来：{e} —— 出界的那一手没有主体可说"),
            ),
        }
    }

    // The other input path: whatever the pointer is over becomes a picture. Blocked
    // rather than failed when the pointer rests on something that cannot be grabbed
    // (the desktop itself, or a window another process owns).
    {
        match capture::window_under_cursor() {
            None => check(
                "the window under the cursor",
                Check::Blocked,
                format!(
                    "no enumerable window at {:?}",
                    platform_windows::proc::cursor_pos()
                ),
            ),
            Some(w) => {
                let got = capture::pin_window(w.hwnd);
                let seen = got
                    .as_ref()
                    .ok()
                    .and_then(|id| with(|s| s.with_rendered(*id, |f| (f.width, f.height))))
                    .unwrap_or((0, 0));
                let closed = match &got {
                    Ok(id) => with(|s| s.close(&[*id])),
                    Err(_) => 0,
                };
                check(
                    "the window under the cursor",
                    Check::from(got.is_ok() && seen.0 > 0 && seen.1 > 0 && closed == 1),
                    format!(
                        "{} ({}x{}) -> {}x{} closed={}",
                        if w.title.is_empty() {
                            w.app_name.clone()
                        } else {
                            w.title.clone()
                        },
                        w.visible_bounds().w,
                        w.visible_bounds().h,
                        seen.0,
                        seen.1,
                        closed
                    ),
                );
            }
        }
    }

    // §5.15 那一族热区。core 的角落机器自写完起一个调用点都没有，而它的判据全是纯算术——
    // 合成指针、合成时钟、合成修饰键，正是这一格里跑得动的那一半。真指针与真显示器那一半
    // 不在这里判：那要一台有人手、有前台窗的机器，它有自己的门 `--hotcorner`。
    {
        let (verdict, detail) = crate::hot_corner::check();
        check("hot corner dwell and routing", verdict, detail);
    }

    capture::clear();
    out.push(format!("capture summary: {}", capture::monitors_line()));
    out.push(format!(
        "capture selftest: {}",
        match worst {
            Check::Pass => "PASS",
            Check::Blocked => "BLOCKED",
            Check::Fail => "FAIL",
        }
    ));
    (worst, out.join("\n"))
}

/// P75：`--selftest --history` 那一档的 dispatch。
///
/// `settings::startup_history_probe` 内部走 `history_open_in`：`[history].enabled = true`
/// 一侧真调 `History::open(dir)`、disk 长出（或复用）`history.db`；`false` 一侧 `Ok(None)`、
/// 不动 disk。失败 ⇒ `Fail`；启用且成功 ⇒ `Pass`；禁用（走 `Ok(None)`）⇒ `Pass` 且明写"disabled"。
/// 分岔本体不在这里、在 `history_open_in`；本轮的 cargo 单测证人在 `main.rs` 的 refuse 规则
/// （`refusals_reject_history_without_selftest`）。
pub fn history_selftest() -> (Check, String) {
    let dir = crate::settings::history_data_dir();
    match crate::settings::startup_history_probe() {
        Ok(Some(_)) => (
            Check::Pass,
            format!("history selftest: PASS (opened history.db under {dir:?})"),
        ),
        Ok(None) => (
            Check::Pass,
            format!("history selftest: PASS (history disabled; no db touched under {dir:?})"),
        ),
        Err(e) => (
            Check::Fail,
            format!("history selftest: FAIL under {dir:?}: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{not_drawn, plant_fate_for, read_refusal, Frame, PinState};
    use falcon_core::encode::EncodeError;

    /// PRD 910's two shapes are one error to the decoder - both a damaged file and an
    /// unrecognised extension arrive as `Codec`, because an unknown suffix only means the
    /// format had to be guessed from the bytes - so the split lives in this file and is
    /// decided by the name. A refusal that says the wrong half is a user told to go look
    /// for a broken file that is fine, which is why the two arms are graded against each
    /// other and not only against English.
    #[test]
    fn a_file_that_will_not_decode_is_named_by_what_the_name_says() {
        let damaged = read_refusal(
            "C:\\temp\\broken.png",
            &EncodeError::Codec("could not determine image size".into()),
        );
        let unsupported = read_refusal(
            "C:\\temp\\notes.qqq",
            &EncodeError::Codec("Image format not recognized".into()),
        );
        for (said, word, other, name) in [
            (&damaged, "损坏", "不是本程序会读的图片", "broken.png"),
            (&unsupported, "不是本程序会读的图片", "损坏", "notes.qqq"),
        ] {
            assert!(said.contains(word) && said.contains(name), "{said}");
            assert!(!said.contains(other), "{said} answers two questions");
            for english in [
                "io: ",
                "codec failed",
                "unsupported format",
                "not recognized",
            ] {
                assert!(!said.contains(english), "{said} forwards {english:?}");
            }
        }
    }

    /// §10 第 37 项 ①. `session` prints this under `复制失败：` and `保存失败：`, so for
    /// as long as it was English the line a user read was one Chinese prefix wrapped
    /// around a sentence they could not use. The `--pin` gauge is the acceptance; this
    /// is the guard for the runs with no screen attached, and it is written against the
    /// whole sentence because a prefix is not allowed to be the only Chinese part of a
    /// Chinese interface.
    #[test]
    fn an_undrawn_pin_answers_without_an_english_word() {
        let said = not_drawn(7);
        assert!(said.contains("贴图") && said.contains('7'), "{said}");
        assert!(
            !said.chars().any(|c| c.is_ascii_alphabetic()),
            "the sentence still carries Latin letters: {said}"
        );
    }

    /// A file that is simply not there is the third shape line 910 covers, and the one
    /// this function cannot get from a name: the extension is a real one, so only the
    /// operating system's own answer says why.
    #[test]
    fn a_file_that_is_not_there_names_the_file() {
        let said = read_refusal(
            "C:\\temp\\gone.png",
            &EncodeError::Io(std::io::Error::from(std::io::ErrorKind::NotFound)),
        );
        assert!(
            said.contains("gone.png") && said.contains("打不开"),
            "{said}"
        );
        assert!(!said.contains("io: "), "{said}");
    }

    /// A directory under the temp root that no earlier leg of this test left behind.
    /// Same shape as `mask::tests::sandbox`, and only there to keep two doors' files from
    /// reading each other's leftovers.
    fn sandbox(name: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("falconshot-pin-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("sandbox");
        dir
    }

    /// §5.10.2 的那一只手跑在一枚**人造**桌面上，问的是真机问不到的那一件事：撞墙的那一枚不许
    /// 把其余的钉在它身边。真机上这一问跑不了 —— 本仓一枚 `SendInput` 也没有，左键按住拖动进
    /// 不来；而两枚贴图里谁先撞墙由它们此刻各自站在哪儿决定，门不能猜（#261 证人不在就不猜）。
    /// 所以这一枚单测把它钉在算术上：1600x900 的单屏、`MIN_VISIBLE_PX` 24，一枚右缘还剩 110 px
    /// 可走的贴图加 100 px 只能走到 x=1576，而同批那一枚走满 100 px。
    ///
    /// 期望是**写死的数**而不是 `desk.keep_visible` 的回声：让被检的那只手自己算期望，它写成
    /// 什么样这一枚都照样绿（#274/#440 替代尺也要能红）。五臂各问一件事：① 两枚一起走且各自
    /// 被各自的地板接住；② 不在名单里的那一枚被拖时，名单里那一枚一步也不动（§5.9.1 那一手不许
    /// 顺手搬走别人的贴图）；③ 名单里只有一枚时交回的是一枚，而它已经贴在地板上——**交回的仍是
    /// 名单的条数**，不是"看上去动了没有"（动了 0 px 也搬过了，那一格是环境的答案）；④ 一枚不在
    /// 集里的 id 交 0 且什么也不搬；⑤ 关掉名单里的一枚，名单**只**剔掉它 —— 这一问要两枚都在
    /// 名单里才问得出，只有关掉的那一枚在里面时「把它剔出去」与「把整张名单清空」交回的是同一
    /// 个空集。
    ///
    /// 最后一手还读 `pushed`：本档两枚都是 `Volatile`（压根没有像素可画），所以那一格只用来证明
    /// 空手没有偷偷画出一张纹理，不拿它当「纯搬动不重画像素」的证人 —— 那一句在 selftest 那一格
    /// 里有真像素时问过，两枚各答各的（#271 标签不许向读数要它没供的东西）。
    #[test]
    fn a_batch_drag_moves_every_selected_pin_and_the_wall_stops_only_the_one_it_reached() {
        use super::{Desktop, Monitor, PinId, Scale};
        use falcon_core::geometry::{PhysPoint, PhysRect, PhysSize};
        use falcon_core::pin::ImageRef;

        let mut s = PinState::default();
        s.refit_onto(Desktop::from_monitors(vec![Monitor::new(
            PhysRect::new(0, 0, 1600, 900),
            Scale::ONE,
        )]));
        let wall = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(100, 80),
                PhysPoint::new(1490, 50),
                &s.cfg,
            )
            .expect("spawn");
        let open = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(100, 80),
                PhysPoint::new(100, 50),
                &s.cfg,
            )
            .expect("spawn");
        let at = |s: &PinState, id: PinId| s.set.get(id).unwrap().pos_phys.x;

        // ① 两枚一起走：被按住的是**已经快贴墙**的那一枚，位移对它不够、对另一枚够。
        let _ = s.clear_selection();
        let _ = s.select_pin(wall, false);
        let _ = s.select_pin(open, true);
        assert_eq!(s.drag_pin(wall, 100, 0), 2, "名单两枚，这一拖就该搬两枚");
        assert_eq!(at(&s, wall), 1576, "撞墙那一枚该被地板接住");
        assert_eq!(at(&s, open), 200, "撞墙那一枚不许把另一枚钉在身边");

        // ② 拖的是**不在名单里**的那一枚：名单里那一枚一步也不动。
        let _ = s.clear_selection();
        let _ = s.select_pin(wall, false);
        assert_eq!(
            s.drag_pin(open, -40, 0),
            1,
            "不在名单里的一枚就该走 §5.9.1 那一手"
        );
        assert_eq!(at(&s, open), 160);
        assert_eq!(at(&s, wall), 1576, "别人的窗不该被这一拖带走");

        // ③ 名单里只有一枚、且它已经贴在地板上：交回的是名单的条数，不是动了几个像素。
        let _ = s.clear_selection();
        let _ = s.select_pin(wall, false);
        assert_eq!(s.drag_pin(wall, 100, 0), 1, "名单一枚，交回一枚");
        assert_eq!(at(&s, wall), 1576, "地板没有给它地方，它就该留在原地");
        assert_eq!(at(&s, open), 160, "没被点名的一枚这一步也不许动");

        // ④ 一扇已经关掉的窗剩下的那一次鼠标事件：交 0、什么也不搬、也不画纹理。
        let moved = s.drag_pin(9_000_001, 5, 5);
        assert_eq!(moved, 0, "不在集里的一枚没有「搬成」可言");
        assert_eq!(
            (at(&s, wall), at(&s, open)),
            (1576, 160),
            "{moved} 却搬动了谁"
        );
        assert!(
            s.pushed.is_empty(),
            "两枚 Volatile 贴图一张纹理也不该被推出去：{:?}",
            s.pushed
        );

        // 关掉的窗剩下的鼠标事件不许在名单里留下指着一枚不存在的贴图的行。这一问要**两枚都在
        // 名单里**才问得出：只有关掉的那一枚在里面时，「把它剔出去」和「把整张名单清空」交回的
        // 是同一个空集，那一格就是枚枚都绿的一把尺（#274/#440 替代尺也得能红）。
        let _ = s.clear_selection();
        let _ = s.select_pin(wall, false);
        let _ = s.select_pin(open, true);
        let _ = s.close(&[wall]);
        assert_eq!(
            s.selected_ids(),
            vec![open],
            "关掉名单里的一枚，另一枚该留在名单里"
        );
    }

    /// §10 第 47 项 ②: the pin door's half of what 第 46 项 ① proved of the mask door.
    /// PRD 447 says the user **应提醒**背景**将**被填充, so the judgment is about
    /// *order*: at the moment the sentence is spoken, the file must not exist yet, and the
    /// sentence itself must not create it. The second half is that the two sentences
    /// differ only in tense - one user, one door, no two ways of saying it.
    ///
    /// This does not go through [`PinState::add`]: `sync` hands the texture to Qt's image
    /// provider and there is no `QGuiApplication` under `cargo test`. The frame is placed
    /// straight into `rendered`, which *is* "how this pin currently shows itself" and is
    /// the one slot both the reminder and the save read - so the ordering under test is
    /// the ordering the product has, while the pixels→texture leg stays the gauge's
    /// (`pin saved png`, which compares the pin's bytes against the mask's).
    #[test]
    fn the_pin_door_says_it_before_the_file_exists() {
        let dir = sandbox("pre-notice");
        let mut s = PinState::default();
        s.rendered.insert(1, Frame::new(8, 4).expect("frame"));
        let total = 8 * 4;

        let png = dir.join("pin.png");
        assert!(
            s.remind_before_save(1, &png.to_string_lossy(), "")
                .is_empty(),
            "PNG was told it loses pixels"
        );
        assert_eq!(s.pre_notice, "");
        assert!(!png.exists(), "a reminder wrote a file");

        let jpg = dir.join("pin.jpg");
        let pre = s.remind_before_save(1, &jpg.to_string_lossy(), "");
        assert!(pre.contains(&format!("贴图里有 {total} px 透明")), "{pre}");
        assert!(pre.contains("将被填成白色"), "{pre}");
        assert!(pre.contains("存成 PNG"), "{pre}");
        assert!(!jpg.exists(), "the reminder itself wrote {jpg:?}");
        assert_eq!(s.pre_notice, pre, "窗口那一格与返回值不是同一句");

        let said = s.save_image(1, &jpg.to_string_lossy(), "");
        assert!(
            jpg.exists(),
            "the write did not follow the notice: {said:?}"
        );
        let post = pre.replace("将被填成白色", "已填成白色");
        let said = said.expect("the notice was followed by a refusal");
        assert!(
            said.ends_with(&post),
            "两句话差的不止时态：前 {pre:?} 后 {said:?}"
        );

        // A name this app cannot give a format answers nothing, and takes the earlier
        // sentence off the window instead of leaving a JPG warning about a PNG.
        let odd = dir.join("pin.qqq");
        assert!(s
            .remind_before_save(1, &odd.to_string_lossy(), "")
            .is_empty());
        assert_eq!(
            s.pre_notice, "",
            "a refused name kept a stale notice on the window"
        );
        assert!(!odd.exists());

        // The same holds for a pin that never drew: the refusal is the window's answer,
        // not last write's.
        s.pre_notice = "a notice from somewhere else".to_string();
        assert!(s
            .remind_before_save(99, &jpg.to_string_lossy(), "")
            .is_empty());
        assert_eq!(
            s.pre_notice, "",
            "an undrawn pin kept someone else's notice"
        );
        assert!(s.save_image(99, &jpg.to_string_lossy(), "").is_err());
        assert_eq!(s.pre_notice, "", "a refused save left a notice behind");
    }

    /// §8.5's door, on a machine that cannot unplug its own monitor. A pin was
    /// left on a second screen and that screen is gone: nothing but this call
    /// brings the pin back, because the position the user last saw is now off the
    /// edge of what exists. The asserted number is the clamp's own answer for
    /// these two rectangles - bounds right edge 1600 less `MIN_VISIBLE_PX` 24 -
    /// so the row says the door reached the clamp, which is the part that was
    /// never called, rather than agreeing with it.
    #[test]
    fn a_pin_on_the_monitor_that_just_went_away_comes_back_inside_what_is_left() {
        use super::{notice, Desktop, Monitor, Refit, Scale};
        use falcon_core::geometry::{PhysPoint, PhysRect, PhysSize};
        use falcon_core::pin::ImageRef;

        let mon = |x, y| Monitor::new(PhysRect::new(x, y, 1600, 900), Scale::ONE);
        let two = Desktop::from_monitors(vec![mon(0, 0), mon(1600, 0)]);
        let one = Desktop::from_monitors(vec![mon(0, 0)]);

        let mut s = PinState::default();
        // 100x80 at x=2500: well inside the two-screen desktop, wholly off the
        // one that is left.
        let id = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(100, 80),
                PhysPoint::new(2500, 50),
                &s.cfg,
            )
            .expect("spawn");

        // The table arrives from the platform, so the first refit is the arm that
        // installs a layout and moves nothing: a pin already inside it must come
        // back with the position it had, not a fresh clamp.
        let first = s.refit_onto(two.clone());
        assert!(
            first.changed && first.moved == 0 && first.total == 1,
            "{first:?}"
        );
        assert_eq!(
            s.set.get(id).unwrap().pos_phys.x,
            2500,
            "a pin inside the new bounds was pushed anyway"
        );
        assert!(notice(first).is_none(), "nothing moved, so no hint was due");

        let second = s.refit_onto(one.clone());
        assert!(second.changed && second.moved == 1, "{second:?}");
        let p = s.set.get(id).unwrap();
        assert_eq!(p.pos_phys, PhysPoint::new(1576, 50), "{p:?}");
        assert_eq!(
            p.size_phys,
            PhysSize::new(100, 80),
            "a screen change resized the picture instead of moving it"
        );
        let said = notice(second).expect("one pin moved and nobody was told");
        assert!(said.contains("1 张贴图"), "{said}");
        assert!(
            !said.chars().any(|c| c.is_ascii_alphabetic()),
            "一句中文提示里混进了英文：{said}"
        );

        // The same table twice is the arm a call that changed nothing takes, and it
        // must not read as a screen full of moved pins: the sentence is owed by
        // `moved`, not by the fact that something was re-read.
        let third = s.refit_onto(one);
        assert_eq!(
            third,
            Refit {
                changed: false,
                moved: 0,
                total: 1
            }
        );
        assert!(notice(third).is_none());
    }

    /// §5.5.11 步骤 3 的那一判第一次有一枚**只读**的形状：两臂各问一次，行里四段都得由这一枚
    /// 自己印出来。判据写死成整行相等而不是 `contains`，因为门那一格引的就是这一整行 —— 少一段、
    /// 多一段、或同一问第二次答得不一样，都是门在引一枚它读不懂的读数（#239 那一族）。
    ///
    /// 最后一手判的是"只读"这两个字的反面：跑完两问，两枚贴图的 `live` 都必须还是 `true`。
    #[test]
    fn a_probe_names_only_the_pin_that_could_cover_the_window() {
        use falcon_core::geometry::{PhysPoint, PhysRect, PhysSize};
        use falcon_core::pin::ImageRef;

        let mut s = PinState::default();
        let near = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(40, 30),
                PhysPoint::new(20, 20),
                &s.cfg,
            )
            .expect("near");
        let far = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(40, 30),
                PhysPoint::new(900, 700),
                &s.cfg,
            )
            .expect("far");

        // 与上面那一格同一枚矩形：挑出来的必须是同一枚，判据不能一问一答两个样。
        assert_eq!(
            s.dialog_probe(Some(PhysRect::new(45, 35, 300, 200))),
            format!("rect=45,35,300,200 covering={near} pins=2 enabled=0 layers=0 hidden=-"),
            "探针那一行说的不是这一判自己挑出来的那几枚"
        );
        // 读不到矩形 ⇒ `covering=-`，而**不是**"整批都算"：那一臂属于
        // [`Self::dialog_opened`]，探针没有藏东西的权力，也就没有替它保守的权力。
        assert_eq!(
            s.dialog_probe(None),
            "rect=- covering=- pins=2 enabled=0 layers=0 hidden=-",
            "没有矩形时探针自己编了一枚覆盖关系"
        );
        assert!(
            s.view_data(near).live,
            "一枚只读的探针把可能遮挡的那枚弄走了"
        );
        assert!(
            s.view_data(far).live,
            "一枚只读的探针把压不住的那枚也弄走了"
        );
    }

    /// 探针与账是两问：藏已经发生了（`layers=1 hidden=<id>`），而这一枚问的矩形压不住任何贴图
    /// ⇒ `covering=-`。两个数同印在一行里正是门要引的形状；再问一枚压得住的，`covering` 必须
    /// 跟上而账**一动不动** —— 探针若自己 +1 层，门引的就不再是产品此刻的账。
    #[test]
    fn a_probe_reports_the_account_it_did_not_touch() {
        use falcon_core::geometry::{PhysPoint, PhysRect, PhysSize};
        use falcon_core::pin::ImageRef;

        let mut s = PinState::default();
        let id = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(40, 30),
                PhysPoint::new(20, 20),
                &s.cfg,
            )
            .expect("pin");
        s.dialog_opened(true, None);

        assert_eq!(
            s.dialog_probe(Some(PhysRect::new(900, 700, 60, 40))),
            format!("rect=900,700,60,40 covering=- pins=1 enabled=1 layers=1 hidden={id}"),
            "账上藏着的那枚，被探针按几何算成了压得住"
        );
        assert_eq!(
            s.dialog_probe(Some(PhysRect::new(30, 25, 60, 40))),
            format!("rect=30,25,60,40 covering={id} pins=1 enabled=1 layers=1 hidden={id}"),
            "探针自己动过账：第二次问的 layers 不再是第一次那一枚"
        );
        assert_eq!(s.link.dialogs(), 1, "一枚只读的探针多开了一层");
    }

    /// §5.5.11 步骤 3 在这一层真正落地的那一格：一扇对话框的出现改的是 [`PinState::link`]，
    /// 而 `live` 读的就是它 —— 贴图窗口那一侧仍然只有 `visible: view.live && …` 一行绑定，
    /// 藏与放没有第二处地方可以发生。
    ///
    /// 两枚贴图压在两枚不同的位置上，所以「谁遮挡」有唯一一枚答案；判的是**那一枚**被藏、
    /// 而不是两枚都被藏（过藏）或一枚也不藏（没接上）。最后一手判的是这一趟的账长什么样：
    /// 那三枚数由 `dialog_line` 印进归档，标签才有权引用它们（#239 那一族）。
    #[test]
    fn a_dialog_hides_only_the_pin_that_could_cover_it() {
        use falcon_core::geometry::{PhysPoint, PhysRect, PhysSize};
        use falcon_core::pin::ImageRef;

        let mut s = PinState::default();
        let near = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(40, 30),
                PhysPoint::new(20, 20),
                &s.cfg,
            )
            .expect("near");
        let far = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(40, 30),
                PhysPoint::new(900, 700),
                &s.cfg,
            )
            .expect("far");

        // 只压住 `near` 右下角的一小块：按钮在那一块上，所以这一枚必须藏。
        let dlg = PhysRect::new(45, 35, 300, 200);
        assert_eq!(
            s.dialog_opened(true, Some(dlg)),
            vec![near],
            "藏下来的不是可能遮挡的那几枚"
        );
        assert!(!s.view_data(near).live, "藏起来的贴图还报自己 live");
        assert!(s.view_data(far).live, "压不住对话框的那一枚被一起藏了");
        assert_eq!(
            s.dialog_line(),
            format!("enabled=1 layers=1 hidden={near}"),
            "这一趟的账与交回的名单不是同一批"
        );

        assert_eq!(
            s.dialog_closed(),
            vec![near],
            "最后一层没把藏过的那枚交回来"
        );
        assert!(
            s.view_data(near).live && s.view_data(far).live,
            "放回以后仍有一枚报不 live"
        );
        assert_eq!(s.dialog_line(), "enabled=1 layers=0 hidden=-");
    }

    /// §5.15「隐藏 / 显示 / 切换全部贴图」在这一层落成的那一格：三手改的是 [`PinState::all_hidden`]
    /// 这枚**独立**理由，而 `live` 把它与 [`PinState::link`]（对话框遮挡）、core 的 `group_hidden`
    /// 三件事**分别**读进来。判的核心是独立性：整组藏着的那一枚，被 [`PinState::show_all_pins`]
    /// 放开全局之后**必须仍**不 `live` —— 若这三件事共用同一格理由，这一手会错误地把组藏着的那枚
    /// 放回桌面（#281：一桩事只写一次，别把两桩事挤进同一格）。
    #[test]
    fn show_all_leaves_a_group_hidden_pin_hidden() {
        use falcon_core::geometry::{PhysPoint, PhysSize};
        use falcon_core::pin::ImageRef;

        let mut s = PinState::default();
        let grouped = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(10, 10),
                PhysPoint::new(0, 0),
                &s.cfg,
            )
            .expect("grouped");
        let lone = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(10, 10),
                PhysPoint::new(500, 500),
                &s.cfg,
            )
            .expect("lone");
        assert!(
            s.view_data(grouped).live && s.view_data(lone).live,
            "起手两枚都该是活的"
        );

        // 归组 + 藏整组：只有组里那枚不 live。
        let g = s.create_group("甲").expect("group");
        s.select_pin(grouped, true).expect("select");
        s.assign_selection_to_group(Some(g)).expect("assign");
        s.toggle_group_shown(g).expect("hide group");
        assert!(!s.view_data(grouped).live, "整组隐藏没把组里那枚藏起来");
        assert!(s.view_data(lone).live, "整组隐藏不该动组外那枚");

        // 全局隐藏：两枚都不 live，且这一手答「已收起」。
        assert!(s.hide_all_pins(), "hide_all 该答「已收起」");
        assert!(
            !s.view_data(grouped).live && !s.view_data(lone).live,
            "全局隐藏没把每扇窗都藏起来"
        );

        // 全局放开：**只有**组外那枚回来，组里那枚仍被 `group_hidden` 按住 —— 这就是独立性。
        assert!(!s.show_all_pins(), "show_all 该答「已放开」");
        assert!(
            !s.view_data(grouped).live,
            "show_all 替整组隐藏放开了它不该放的那枚：两桩事共了一格"
        );
        assert!(s.view_data(lone).live, "show_all 没把组外那枚放回来");

        // 切换两下：lone 先进收起、再回放开，返回值与 live 一致。
        assert!(s.toggle_all_pins(), "第一下切换该进入收起");
        assert!(!s.view_data(lone).live, "切换进入收起后组外那枚仍 live");
        assert!(!s.toggle_all_pins(), "第二下切换该回到放开");
        assert!(s.view_data(lone).live, "切换回到放开后组外那枚仍不 live");

        // 收尾：把组放回 —— 独立性只在两枚理由同时压着时可见，放回组之后那枚该 live。
        s.toggle_group_shown(g).expect("show group");
        assert!(s.view_data(grouped).live, "放回组后组里那枚仍不 live");
    }

    /// 运行期**普通**的那一臂：Qt 把对话框标成可见要比 Win32 真造出那扇 `#32770` 早约
    /// 400 ms，所以事件到达时没有矩形可读 —— 那一臂藏的是集里全部，这是「可能遮挡」最保守
    /// 的读法（多藏几枚用户看得见，而且一关就都回来；按钮点不到的原因看不见）。
    ///
    /// 第二臂是步骤 1 那一格关着：一枚也不藏，而 `enabled` 必须是**读到的那一份**印在账里，
    /// 否则「关着」与「没人叫过这枚」在两枚数上长得一样（#281 那一族要分开的正是这两件事）。
    #[test]
    fn a_dialog_with_no_rect_yet_hides_everything_still_in_the_set() {
        use falcon_core::geometry::{PhysPoint, PhysSize};
        use falcon_core::pin::ImageRef;

        let mut s = PinState::default();
        let a = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(10, 10),
                PhysPoint::new(0, 0),
                &s.cfg,
            )
            .expect("a");
        let b = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(10, 10),
                PhysPoint::new(500, 500),
                &s.cfg,
            )
            .expect("b");

        assert_eq!(
            s.dialog_opened(true, None),
            vec![a, b],
            "读不到矩形时藏的不是集里全部"
        );
        assert!(
            !s.view_data(a).live && !s.view_data(b).live,
            "藏起来的那两枚里有一枚还报 live"
        );
        assert_eq!(s.dialog_closed(), vec![a, b]);

        assert!(
            s.dialog_opened(false, None).is_empty(),
            "步骤 1 关着，联动却替用户把它打开了"
        );
        assert_eq!(
            s.dialog_line(),
            "enabled=0 layers=0 hidden=-",
            "关着时连层数都不记，这一行要说得出是开关关着而不是没人开过对话框"
        );
        assert!(s.view_data(a).live && s.view_data(b).live);
    }

    /// §5.9.17「失效前不得删除」+ §6.2「按策略自动清理」是同一枚文件的两种命运，而真机的那一趟
    /// 每次只能遇见其中**一面**（这一枚会话要么读得开剪贴板、要么读不开；本档一个字也不写它，所以
    /// `Some(true)` 在这里根本不可达）。一面都没被推翻过的判据，等于一枚只会答「是」的尺 —— 所以
    /// 这一格把三面逐枚摆平了量：`#274`/`#461` 那条「换到一面读得出的尺」在本轮的形状。
    #[test]
    fn the_planted_file_has_three_faces_and_only_two_of_them_are_alright() {
        // 被点名：那句禁令的字面意思是「不得删除」，所以活着 + sweep 自己承认留下，才算答对。
        assert!(
            plant_fate_for(Some(true), true, true),
            "被点名、活着、被承认留下"
        );
        assert!(
            !plant_fate_for(Some(true), false, false),
            "被点名却被删掉了，这一面必须答否（那一次复制的临时文件已经成了一枚死引用）"
        );
        assert!(
            !plant_fate_for(Some(true), true, false),
            "文件还在盘上而 sweep 说它没留下：这一枚是 sweep 的账与盘上的账不一致"
        );
        assert!(
            !plant_fate_for(Some(true), false, true),
            "sweep 说它留下了而盘上没有了：留下的那一枚不能只活在 sweep 的返回值里"
        );

        // 没被点名：这一面是「按策略自动清理」，它必须走；不走就是那句策略没人执行。
        assert!(
            plant_fate_for(Some(false), false, false),
            "没被点名、已被清走"
        );
        assert!(
            !plant_fate_for(Some(false), true, true),
            "剪贴板没点名它，sweep 却把它留下了 —— 「按策略自动清理」在这一枚上没人读"
        );

        // 读不开：unknown 不是「没被引用」。这一面要求的命运和上一面**正好相反**，所以三面合成一枚
        // `claimed.unwrap_or(false)` 的那一枚写法是必须被读成否的（#453）。
        assert!(plant_fate_for(None, true, true), "读不开时一枚也不许动");
        assert!(
            !plant_fate_for(None, false, false),
            "读不开却删了东西：把 unknown 当成空集的那一枚 bug，在这一面上就是删了用户的临时文件"
        );
        assert!(
            !plant_fate_for(None, true, false),
            "读不开、文件活着、sweep 却不承认它活着：这一枚账也不自洽"
        );

        // 同一枚 (alive, kept) 读数在三面下必须给出**不同**的答案，否则那枚 `claimed` 就是装饰。
        assert!(
            plant_fate_for(None, true, true)
                && !plant_fate_for(Some(false), true, true)
                && plant_fate_for(Some(true), true, true),
            "盘上读数一样而剪贴板的答案不同 ⇒ 三面合成了一面"
        );
    }

    /// §5.10.4 的四只手（重置 / 关闭 / 销毁）加 §5.9.18 的两笔账（影响数量 / 撤销），在这一格
    /// 各问一件真机问不到的事：批量「关闭」那一臂在门上会把发号的窗自己关掉（`syncPins` 拆掉
    /// 它的 delegate），而 `trash` 里那一叠**像素**只有本档看得见 —— 撤销回来的那扇窗有没有
    /// 图，真机那一侧只读得出「有几扇窗在场」。所以这一格的证人只有这一格，问的是算术与账，
    /// 不是像素（#271：标签不许向读数要它没供的东西；反过来真机那一趟已经答过的问题这里也不
    /// 重答一遍，#281）。
    ///
    /// 本档三枚 / 四枚贴图全是 `ImageRef::Volatile`：`sync` 在这种贴图上没有像素可画，只会
    /// 在 [`Self::frames`] 里留一个空位 —— 而 `frames` 正是销毁那一只手要搬进 `trash_frames`
    /// 的那一只袋，所以第六臂**手工**给它塞一张帧，把那一句量出来，而不是指望 `sync` 画出来。
    ///
    /// 七臂各问一件事，每臂都能单独红：① 名单为空时那一手只碰被点中的那一枚，且**还回它围着
    /// 站的那枚中心**（core 那一格写着 "a reset is not a move"，而转开缩放的那一只手本来就围着
    /// 中心转，所以这一格量的是「落点回到进门那一格」——P134 之前这里量的是左上角不许动，
    /// 那一枚尺与真机上 `(320-288)/2` 那一滑正好相反）；② 批量那一手交回名单的
    /// 条数，名单之外的一枚原地留着；③ 名单不被这一手用掉（§5.10.4 没说名单是一次性的）；
    /// ④「影响数量」对三种点名给出三个互不相同的数（2 / 1 / 0）—— 只会答一个数的尺在这一枚
    /// 上必须红（#274）；⑤ 批量「关闭」带走整张名单、把名单剔干净、一扇已关的窗剩下的那一次
    /// 点击交 0；⑥ 销毁把像素搬走而不是丢掉，撤销把它们连同 z 序一起还回来，而一次交 0 的空手
    /// 不许把这笔欠账花掉；⑦ 撤销只认最近那一手。
    ///
    /// 期望里没有一处写死缩放：`home` 从 [`PinConfig`] 读（P64 起整段 `[pin]` 由配置文件
    /// 送进运行期，#261/#455），本档把它转开 30 档再问「转回来了没有」，并且先把「转得开吗」
    /// 这件事自己量一遍 —— 环境给的默认值若已经在梯子顶端，这一格红在自己身上而不是装作绿。
    #[test]
    fn the_reset_and_retire_hands_answer_the_same_roster_and_the_trash_keeps_the_pixels() {
        use super::{Desktop, Monitor, PinId, Scale};
        use falcon_core::geometry::{PhysPoint, PhysRect, PhysSize};
        use falcon_core::pin::{clamp_zoom, ImageRef};

        let mut s = PinState::default();
        s.refit_onto(Desktop::from_monitors(vec![Monitor::new(
            PhysRect::new(0, 0, 1600, 900),
            Scale::ONE,
        )]));
        let home = clamp_zoom(s.cfg.default_zoom);
        let turned = home + 30;
        assert_ne!(
            turned, home,
            "本档给的「转过」与默认值同一个数：这一格问不出任何重置"
        );

        let a = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(100, 80),
                PhysPoint::new(100, 50),
                &s.cfg,
            )
            .expect("spawn");
        let b = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(100, 80),
                PhysPoint::new(300, 50),
                &s.cfg,
            )
            .expect("spawn");
        let c = s
            .set
            .spawn(
                ImageRef::Volatile,
                PhysSize::new(100, 80),
                PhysPoint::new(500, 50),
                &s.cfg,
            )
            .expect("spawn");
        let zoom = |st: &PinState, id: PinId| st.set.get(id).unwrap().zoom;
        let pos = |st: &PinState, id: PinId| {
            let p = st.set.get(id).unwrap().pos_phys;
            (p.x, p.y)
        };
        let turn = |st: &mut PinState, id: PinId| st.set.get_mut(id).unwrap().set_zoom(turned);

        // 进门那三枚各站的那一格。`turn` 是围着**中心**转的，所以它一挪就把落点挪走了；
        // 「重置」还的必须是同一枚中心，不是左上角——P134 之前还的是左上角，于是每按一次
        // 「重置视图」窗就朝中心那一侧滑掉半次尺寸差（真机证人：`pin window` 那一行答
        // 320x200@624,390 而要求的正是 608,380，差的就是 (+16,+10)）。
        let stood = pos(&s, a);
        turn(&mut s, a);
        turn(&mut s, b);
        turn(&mut s, c);
        let _ = s.set.get_mut(a).unwrap().set_click_through(true);
        assert_eq!(
            (zoom(&s, a), zoom(&s, b), zoom(&s, c)),
            (turned, turned, turned),
            "三枚没有全都转开，后面几臂就在问一个不存在的问题"
        );

        // ① 名单为空 ⇒ 单枚那一臂：只碰被点中的那一枚，只把**视图**还给默认。
        let before = pos(&s, a);
        assert_eq!(s.reset_pin(a), 1, "名单为空时这一手只该碰被点中的那一枚");
        assert_eq!(zoom(&s, a), home, "缩放没还给默认");
        assert!(
            !s.set.get(a).unwrap().click_through,
            "「穿透」是视图的一部分，重置该把它关回去"
        );
        assert_eq!(
            pos(&s, a),
            stood,
            "重置不是搬家：转开那一档围着的那枚中心，重置该照着它把窗放回来"
        );
        assert_ne!(
            before, stood,
            "本档的「转过」根本没挪动落点：这一格问不出「重置还不还位置」，绿得没有对面（#433）"
        );
        assert_eq!(
            (zoom(&s, b), zoom(&s, c)),
            (turned, turned),
            "单枚那一手伸到了名单外"
        );

        // ②③ 名单两枚 ⇒ 同一只手交回的是**名单的条数**，而名单在这之后仍然是那张名单。
        turn(&mut s, a);
        let _ = s.clear_selection();
        let _ = s.select_pin(a, false);
        let _ = s.select_pin(b, true);
        assert_eq!(s.reset_pin(a), 2, "名单两枚，这一手就该重置两枚");
        assert_eq!((zoom(&s, a), zoom(&s, b)), (home, home));
        assert_eq!(zoom(&s, c), turned, "没被点名的一枚不该被这一手抹平");
        assert_eq!(
            s.selected_ids(),
            vec![a, b],
            "重置不是退役：§5.10.4 没说名单要用掉"
        );

        // ④「影响数量」：三种点名给出三个互不相同的数。这一格的证人就是这三个数**不同** ——
        // 任何一只手写成常数（永远 1、永远名单长度）都会在这里红。
        assert_eq!(s.destroy_answer(a), 2, "名单里的一枚按下去带走的是整张名单");
        assert_eq!(s.destroy_answer(c), 1, "名单外的一枚只带走自己");
        assert_eq!(
            s.destroy_answer(9_000_001),
            0,
            "一扇已关的窗剩下的那一次点击，不该报「带走一枚」"
        );
        assert_eq!(s.reset_pin(9_000_001), 0, "不在集里的一枚没有「重置」可言");
        assert_eq!(zoom(&s, c), turned, "交 0 的那一手却把名单外的一枚抹平了");

        // 退役那两只：另一张桌子，四枚贴图，因为这一臂要问的是「场上还剩谁」。
        let mut t = PinState::default();
        t.refit_onto(Desktop::from_monitors(vec![Monitor::new(
            PhysRect::new(0, 0, 1600, 900),
            Scale::ONE,
        )]));
        let put = |t: &mut PinState, x: i32| {
            t.set
                .spawn(
                    ImageRef::Volatile,
                    PhysSize::new(100, 80),
                    PhysPoint::new(x, 50),
                    &t.cfg,
                )
                .expect("spawn")
        };
        let p0 = put(&mut t, 100);
        let p1 = put(&mut t, 300);
        let p2 = put(&mut t, 500);
        let p3 = put(&mut t, 700);

        // ⑤ 批量「关闭」：带走的枚数必须与菜单上印着的那一个数一致（同一张 hand_targets，
        // 两处各数一遍就是 #281，而那一处的错用户看得见）。
        let _ = t.select_pin(p1, false);
        let _ = t.select_pin(p2, true);
        assert_eq!(
            t.destroy_answer(p1),
            2,
            "「影响数量」答 2 而下面带走别的数：印着的数与真被带走的那些不一致"
        );
        assert_eq!(t.close_pin(p1), 2, "批量关闭带走的是整张名单");
        assert_eq!(t.ids(), vec![p0, p3], "名单里那两枚还留在场上");
        assert!(
            t.selected_ids().is_empty(),
            "退役的人不该还在名单里：{:?}",
            t.selected_ids()
        );
        assert_eq!(t.close_pin(p1), 0, "一扇已关的窗剩下的那一次点击");
        assert_eq!(t.close_pin(9_000_001), 0, "ghost 那一手什么也没带走");
        assert_eq!(t.ids(), vec![p0, p3], "交 0 的那一手搬动了谁");

        // ⑥ 销毁：像素搬进 `trash_frames` 而不是丢掉；撤销把它们连同 z 序一起还回来。
        t.frames.insert(p0, Frame::new(8, 4).expect("frame"));
        let _ = t.select_pin(p0, false);
        let _ = t.select_pin(p3, true);
        assert_eq!(t.destroy_answer(p0), 2);
        assert_eq!(
            t.destroy_pin(p0),
            2,
            "销毁带走的枚数该与菜单印着的那一个数一致"
        );
        assert!(t.ids().is_empty(), "整场都该空了：{:?}", t.ids());
        assert_eq!(t.trashed(), 2, "撤销的欠账按枚记");
        assert!(
            !t.frames.contains_key(&p0),
            "位图还留在活账里 —— 销毁就没有把它交出去，也谈不上还"
        );
        assert_eq!(t.destroy_pin(9_000_001), 0, "ghost 那一手没有「带走」可言");
        assert_eq!(
            t.trashed(),
            2,
            "交 0 的那一手把欠账花掉了：撤销只剩一句空话"
        );
        assert_eq!(t.restore_pins(), 2, "还回来的是整手，不是其中一枚");
        assert_eq!(
            t.ids(),
            vec![p0, p3],
            "撤销回来的是原来那一堆，而不是摞在最上面"
        );
        assert_eq!(t.trashed(), 0, "还过的账不能再欠着");
        assert!(
            t.frames.contains_key(&p0),
            "撤销回来的那扇窗没有像素 ⇒ 不是「支持撤销」而是「撤销成一扇坏窗」"
        );
        assert_eq!(
            t.restore_pins(),
            0,
            "空账上再撤销一次该交 0（那一手压根没有撤销可做）"
        );

        // ⑦ 撤销只认最近那一手：槽位记的是「离开时占的那一格」，两批叠着还就会把第二批插进
        // 第一批的位置上。
        let _ = t.clear_selection();
        assert_eq!(
            t.destroy_answer(p0),
            1,
            "名单为空时销毁只带走被点中的那一枚"
        );
        assert_eq!(t.destroy_pin(p0), 1);
        assert_eq!(t.trashed(), 1);
        assert_eq!(t.destroy_pin(p3), 1);
        assert_eq!(t.trashed(), 1, "上一手的账在这一手之前就该结清");
        assert_eq!(t.restore_pins(), 1);
        assert_eq!(t.ids(), vec![p3], "还回来的是最近那一手带走的那一枚");
        assert!(t.set.get(p0).is_none(), "被顶掉的那一手不该还回来");
    }

    /// §5.12 图片分组管理：core 那六只手（create / rename / delete / assign / move / toggle）
    /// 自本项起各有一枚桥侧读者。这一格证的是**桥侧那一层**把它们接得对不对 —— 命名规则、
    /// 「贴图如何处理」那两臂、整组显隐真的改到 `live`、排序透传，四件事各有一枚会翻的断言。
    #[test]
    fn the_group_hands_cross_the_bridge_and_answer_apart() {
        use super::{Desktop, Monitor, Scale};
        use falcon_core::geometry::{PhysPoint, PhysRect, PhysSize};
        use falcon_core::pin::ImageRef;
        let mut t = PinState::default();
        t.refit_onto(Desktop::from_monitors(vec![Monitor::new(
            PhysRect::new(0, 0, 1600, 900),
            Scale::ONE,
        )]));
        let put = |t: &mut PinState, x: i32| {
            t.set
                .spawn(
                    ImageRef::Volatile,
                    PhysSize::new(100, 80),
                    PhysPoint::new(x, 50),
                    &t.cfg,
                )
                .expect("spawn")
        };
        let a = put(&mut t, 100);
        let b = put(&mut t, 300);

        // §5.12.1 命名：空名与重名各失败一次，且失败的话各不一样 —— 三臂合一句就是拿猜测当读数。
        assert_eq!(t.create_group("   ").unwrap_err(), "分组名称不能为空");
        let g1 = t.create_group("甲").expect("create 甲");
        assert_eq!(
            t.create_group("甲").unwrap_err(),
            "已经有一个叫 \"甲\" 的分组"
        );
        // core 的 ASCII 大小写不敏感重名规则（§5.12.3 要的「同级能否重名」那一处定义）：
        let _ = t.create_group("乙").expect("create 乙");
        let _b = t.create_group("B").expect("create B");
        assert!(
            t.create_group("b").is_err(),
            "ASCII 大小写不同的同名该被 core 拒掉"
        );

        // §5.12.2 加入分组：把此刻名单（a、b 两枚）移进甲组，交回的枚数 = 名单长度。
        let _ = t.select_pin(a, false);
        let _ = t.select_pin(b, true);
        assert_eq!(t.assign_selection_to_group(Some(g1)).expect("assign"), 2);
        let rows = t.group_rows();
        assert_eq!(rows[0].3, 2, "甲组此刻该有两枚");
        assert!(rows[0].2, "新建/入组都不动可见性");
        assert!(
            t.view_data(a).live && t.view_data(b).live,
            "入组不该藏起任何一枚"
        );

        // §5.12.4 整组显隐：切换把组内**每一枚**的 live 改到 false，再切回来。扇出走全体这件事
        // 由 live 那一格自己读 core `visible()` 担保，这里只验结果（#281：不在桥侧另数一遍）。
        assert!(
            !t.toggle_group_shown(g1).expect("hide"),
            "隐藏那一手答「此刻已隐藏」"
        );
        assert!(
            !t.view_data(a).live && !t.view_data(b).live,
            "整组隐藏该让两枚都不 live"
        );
        assert!(!t.group_rows()[0].2, "快照要说「已隐藏」");
        assert!(
            t.toggle_group_shown(g1).expect("show"),
            "再切换答「此刻显示中」"
        );
        assert!(
            t.view_data(a).live && t.view_data(b).live,
            "再切换要放回原状态而非一半"
        );

        // §5.12.3 排序：move_group 只改那一串的顺序，不动任何一枚的 live。
        let order_before: Vec<u64> = t.group_rows().iter().map(|r| r.0).collect();
        let last = *order_before.last().unwrap();
        t.move_group(last, -1).expect("up");
        let order_after: Vec<u64> = t.group_rows().iter().map(|r| r.0).collect();
        assert_ne!(order_before, order_after, "上移末组该改变顺序");
        assert!(t.view_data(a).live, "排序不该动可见性");

        // §5.12.3 删除的两臂：Ungroup 只清归属、成员不变；Destroy 连贴图一起带走。
        assert_eq!(t.delete_group(g1, false).expect("ungroup"), 2);
        assert_eq!(t.ids().len(), 2, "退回未分组不动成员");
        assert_eq!(
            t.group_rows().iter().find(|r| r.0 == g1).map(|r| r.3),
            None,
            "甲组已不在那张表里"
        );
        // 新起一组、把这**两枚**放进去，再连贴图一起销毁：成员随组走，`ids` 归零。
        let g3 = t.create_group("丙").expect("create 丙");
        let _ = t.select_pin(a, false);
        let _ = t.select_pin(b, true);
        assert_eq!(t.assign_selection_to_group(Some(g3)).expect("assign 丙"), 2);
        assert_eq!(t.delete_group(g3, true).expect("destroy"), 2);
        assert!(t.ids().is_empty(), "连贴图一起销毁要把两枚都带走");
    }
}
