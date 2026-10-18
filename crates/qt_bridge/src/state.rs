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

use falcon_core::config::{AlphaBg, Pin as PinConfig};
use falcon_core::dialog_link::{self, Hidden};
use falcon_core::encode::{self, EncodeOptions, Format};
use falcon_core::frame::Frame;
use falcon_core::geometry::{DipRect, PhysPoint, PhysRect, PhysSize, Scale};
use falcon_core::pin::{Desktop, Monitor, PinId, PinItem, PinSet};

use crate::capture;
use crate::session::shim;

/// Everything a pin window binds to. Plain fields, because it crosses into a
/// CXX-Qt object one property at a time and has to be readable in a test.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PinViewData {
    /// False once the pin has left the set - the window hides itself. Also false while
    /// a file save dialog is on the screen and this pin was one of 那几张可能遮挡它的
    /// 贴图（§5.5.11 步骤 3）：藏与放回都只改这一格，所以窗口那一侧仍然只有一行绑定。
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
    pushed: HashMap<PinId, u64>,
    revisions: HashMap<PinId, u32>,
    /// §5.5.11 步骤 3/4 的账：此刻为一张文件保存对话框藏起来的贴图，以及叠着开了几层。
    /// P124 之前只有 [`crate::config::Output::hide_pins_on_dialog`] 那枚键和一个启动横幅，
    /// 这一格把它变成运行期真正生效的那一件事：[`Self::view_data`] 的 `live` 读它，
    /// 于是藏与放只有**一枚**权威，而窗口的可见性仍然是 `PinWindow.qml` 那一行绑定。
    link: Hidden,
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
        let size = PhysSize::new(frame.width, frame.height);
        let id = self
            .set
            .spawn(falcon_core::pin::ImageRef::Volatile, size, at, &self.cfg)
            .map_err(|e| e.to_string())?;
        self.frames.insert(id, frame);
        self.sync(id);
        Ok(id)
    }

    /// The fields that decide the pixels. Position, zoom and opacity are absent
    /// on purpose: they decide the window.
    fn content_key(p: &PinItem) -> u64 {
        let region = p.visible_rect();
        let parts: [i64; 10] = [
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
        ];
        parts.into_iter().fold(0xcbf2_9ce4_8422_2325, |h, v| {
            (h ^ (v as u64)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    /// Render, encode and hand Qt the result - but only if the pixels moved.
    /// Returns whether a new texture went out.
    pub fn sync(&mut self, id: PinId) -> bool {
        let key = match self.set.get(id) {
            Some(p) => Self::content_key(p),
            None => {
                self.pushed.remove(&id);
                self.revisions.remove(&id);
                return false;
            }
        };
        if self.pushed.get(&id) == Some(&key) {
            return false;
        }

        let mut shown: Option<Result<Frame, String>> = None;
        if let (Some(p), Some(src)) = (self.set.get(id), self.frames.get(&id)) {
            shown = Some(
                p.render_shown(src)
                    .map(|f| p.paint_alpha_bg(f))
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
        PinViewData {
            live: !self.link.hidden().contains(&id),
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
        }
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
    pub fn close(&mut self, ids: &[PinId]) -> usize {
        let n = self.set.close(ids);
        for id in ids {
            self.frames.remove(id);
            self.rendered.remove(id);
            self.pushed.remove(id);
            self.revisions.remove(id);
            shim::drop_frame(*id);
        }
        n
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

    /// §5.9.17 - the pin out to a file, through the same owner the mask's 另存为 uses
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
        self.add(frame, at)
    }

    /// §5.8.2 - whatever the user copied becomes a pin. Deciding *what it was* is
    /// `core::clip::classify`'s job, already tested; this only moves bytes.
    pub fn add_from_clipboard(&mut self, at: PhysPoint) -> Result<PinId, String> {
        let contents = platform_windows::clip::read().map_err(|e| e.to_string())?;
        if contents.is_empty() {
            return Err("剪贴板是空的".into());
        }
        let payload = {
            let facts = contents.facts();
            falcon_core::clip::classify(&facts).map_err(|e| e.to_string())?
        };
        let frame = match payload {
            (falcon_core::clip::ClipKind::Image, falcon_core::clip::ClipPayload::Image(f)) => f,
            (falcon_core::clip::ClipKind::Files, falcon_core::clip::ClipPayload::Files(paths)) => {
                let first = paths.first().ok_or("剪贴板里的文件列表是空的")?;
                encode::decode_file(first).map_err(|e| e.to_string())?
            }
            (falcon_core::clip::ClipKind::Colour, falcon_core::clip::ClipPayload::Colour(rgba)) => {
                // §5.12: a colour swatch is a pin of one pixel, which the text
                // card's renderer will draw properly in Phase 2.
                Frame::filled(64, 64, rgba).map_err(|e| e.to_string())?
            }
            (_, falcon_core::clip::ClipPayload::Empty) => Err("剪贴板没有可贴图的内容")?,
            (kind, _) => Err(format!("{kind:?} 贴图还未实现（Phase 2）"))?,
        };
        self.add(frame, at)
    }

    fn note(&mut self, message: String) {
        self.problems.push(message);
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

        // Closing takes the pin and its texture out together.
        let closed = with(|s| s.close(&[id, board]));
        check(
            "closing drops the pixels",
            closed == 2 && shim::self_check(id).contains("null=1"),
            format!("closed={closed} {}", shim::self_check(id)),
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
            .and_then(|id| with(|s| s.with_rendered(*id, |f| (f.width, f.height))))
            .unwrap_or((0, 0));
        let closed = match &got {
            Ok(id) => with(|s| s.close(&[*id])),
            Err(_) => 0,
        };
        check(
            "a frozen region becomes a pin",
            Check::from(got.is_ok() && seen == (48, 32) && closed == 1),
            format!(
                "{} -> {}x{} closed={}",
                match &got {
                    Ok(id) => format!("pin {id}"),
                    Err(e) => e.clone(),
                },
                seen.0,
                seen.1,
                closed
            ),
        );
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
    use super::{not_drawn, read_refusal, Frame, PinState};
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
}
