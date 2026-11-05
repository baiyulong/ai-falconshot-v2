//! The screenshot history: `history.db` next to one PNG per record (§6.2).
//!
//! 计划 §6.2 fixes the split. SQLite holds *metadata and a thumbnail*; the
//! full-resolution picture is a file under `history/<YYYY-MM-DD>/<id>.png`, and
//! the row points at it through a path that is **relative to the data
//! directory**. That is what makes PRD §6.2's "the user may move the data
//! directory" a settings change rather than a migration: every path stored in
//! the database still means the same thing after the folder moves.
//!
//! Three rules follow from that split:
//!
//! * Space is counted from the files, not from an estimate (§8.7 promises the
//!   settings page a truthful number), so [`History::usage`] and the cleaner
//!   both ask the filesystem how big each original turned out.
//! * Automatic cleanup must spare locked records (PRD §5.14.4), but a *manual*
//!   deletion is the user asking, so [`History::delete`] does not consult the
//!   lock. They are separate functions precisely so the UI cannot confuse them.
//! * A record whose original is gone is still a record. [`History::frame`] falls
//!   back to the thumbnail instead of failing, which is what §5.14.3's "recover
//!   the cancelled capture" needs when `keep_originals` is off.

use crate::capture::{now_ms, ScreenSnapshot};
use crate::config::History as Settings;
use crate::encode::{self, Format};
use crate::frame::Frame;
use crate::geometry::{PhysPoint, PhysRect};
use crate::naming::LocalStamp;
use rusqlite::{params, Connection, OptionalExtension};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HistoryError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("coding: {0}")]
    Encode(#[from] crate::encode::EncodeError),
    #[error("history.db is from a newer version (schema {0})")]
    Schema(i32),
    #[error("record {0} is no longer there")]
    Missing(i64),
    /// [`stamp_row`]'s first refusal: the row asks to be painted onto a rectangle
    /// with no pixels in it. Not folded into [`Self::OffSnapshot`] because an
    /// empty rect is a *broken record*, while an off-screen one is a record the
    /// current monitor layout no longer covers — a user who hears "no monitor"
    /// about the former goes looking for a display that is not the problem.
    #[error("record rect {0} is empty; there is nothing to stamp")]
    EmptyRect(PhysRect),
    /// [`stamp_row`]'s second refusal: the desktop the caller froze has no
    /// monitor the row's rect overlaps (the record was made on a display that is
    /// now off, or the layout moved).
    #[error("record rect {0} touches no monitor of this snapshot")]
    OffSnapshot(PhysRect),
    #[error("pixels: {0}")]
    Pixels(#[from] crate::frame::FrameError),
}

/// `source_kind` in §6.2 — how the picture came to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Region,
    Window,
    Element,
    Manual,
    Repeat,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Region => "region",
            Source::Window => "window",
            Source::Element => "element",
            Source::Manual => "manual",
            Source::Repeat => "repeat",
        }
    }

    pub fn parse(raw: &str) -> Option<Source> {
        [
            Source::Region,
            Source::Window,
            Source::Element,
            Source::Manual,
            Source::Repeat,
        ]
        .into_iter()
        .find(|s| s.as_str() == raw)
    }
}

/// One row, as the UI sees it. `path` is relative to the data directory, and
/// `None` when the original was never written (`keep_originals` off, §6.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub id: i64,
    pub captured_at: i64,
    pub monitor_id: String,
    pub rect: PhysRect,
    pub source: Source,
    pub path: Option<String>,
    pub thumb: Option<Vec<u8>>,
    pub cancelled: bool,
    pub locked: bool,
    pub ocr_text: Option<String>,
    pub note: String,
}

/// What a capture contributes to the history other than its pixels.
#[derive(Clone, Copy, Debug)]
pub struct CaptureMeta<'a> {
    pub at_ms: i64,
    /// Minutes east of UTC, for the dated folder (§6.2). The caller supplies it
    /// so a test can put a record on a chosen day, and so the store never reads
    /// a clock the platform has not told it about.
    pub offset_min: i32,
    pub monitor_id: &'a str,
    pub rect: PhysRect,
    pub source: Source,
    /// PRD §5.14.1: with "save every capture" on, an Esc-cancelled selection is
    /// still a record — flagged, not omitted (§5.14.3).
    pub cancelled: bool,
}

/// The three retention limits of PRD §5.14.1 / §5.14.4. Zero disables that
/// condition, matching `config::History`.
#[derive(Clone, Copy, Debug)]
pub struct Retention {
    pub max_items: usize,
    pub max_age_ms: i64,
    pub max_bytes: u64,
    pub exempt_locked: bool,
}

impl Retention {
    pub fn from(cfg: &Settings) -> Self {
        Self {
            max_items: cfg.max_items as usize,
            max_age_ms: cfg.max_age_days as i64 * 86_400_000,
            max_bytes: cfg.max_size_mb as u64 * 1024 * 1024,
            exempt_locked: cfg.exempt_locked,
        }
    }

    /// PRD §8.7: has the history reached one of its caps, so the user deserves to
    /// be told he can clean it or raise the limit?
    ///
    /// Judged here, not at the call site, because it re-reads the *same* cap
    /// semantics [`History::cleanup`] walks with: a zero means "this limit is off"
    /// in both, and a second spelling of that rule is how one config number ends
    /// up meaning two things.
    ///
    /// The age cap is not asked about: [`Usage`] carries no timestamps, so "the
    /// oldest row is past `[history] max_age_days`" is a fact about rows this
    /// cannot see — and inventing a yes from the byte count would be a guess.
    pub fn at_cap(&self, usage: &Usage) -> bool {
        (self.max_items > 0 && usage.records as usize >= self.max_items)
            || (self.max_bytes > 0 && usage.total_bytes() >= self.max_bytes)
    }
}

/// Space taken, per PRD §6.2 ("show what the history is using").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub records: u64,
    pub original_bytes: u64,
    pub thumb_bytes: u64,
    pub db_bytes: u64,
}

impl Usage {
    pub fn total_bytes(&self) -> u64 {
        self.original_bytes + self.thumb_bytes + self.db_bytes
    }
}

/// 一次压实问过之后手上的三种答法之一（见 [`History::compact`]）。
///
/// 数字全部当场从 pragma 与文件系统问回来，本档不替它们编（#455）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Compacted {
    /// 空洞还没大到值得重写整本文件，`history.db` 一个字节也没动。
    /// `free_bytes` 是当场 `freelist_count × page_size` 的那一串空页。
    Skipped { free_bytes: u64 },
    /// 真的重写了库并把 `-wal` 截干。`reclaimed` 是 `history.db` 自己前后少掉的字节数，
    /// 与 [`Freed::bytes`]（那是删掉的那些**文件**与缩略图）是两笔各自独立的账。
    Done { free_bytes: u64, reclaimed: u64 },
    /// 这一步没跑成。删除本身已经成了，所以这一臂**不**冒充一次失败的删除（见
    /// [`History::compact`] 头上那条），它只把原因交回调用点那句话。
    Refused { reason: String },
}

/// What one delete operation gave back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Freed {
    pub ids: Vec<i64>,
    pub bytes: u64,
    /// 行交回之后，`history.db` **这枚文件自己**那一步的答法（§6.2 存储规则）。
    /// `None` = 这次一行也没删成，于是压根没问这一问。
    pub compacted: Option<Compacted>,
}

pub const DB_FILE: &str = "history.db";
pub const PICTURES_DIR: &str = "history";
pub const SCHEMA_VERSION: i32 = 1;
/// §6.2 budgets ≈15 KB for a 256 px JPEG; this is the quality that lands there.
pub const THUMB_QUALITY: u8 = 80;

/// 压实门的第一条：空洞至少要有这么大才动手（[`History::compact`]）。
/// 定在这一量级是因为 VACUUM 的代价按**留下的**那部分算，而不按空洞算 —— 为了几百 KB
/// 的空洞重写一本几十 MB 的库是赔本生意，而 §5.14.4 的删除是用户在等的动作。
pub const COMPACT_MIN_FREE_BYTES: u64 = 1_048_576;
/// 压实门的第二条：空洞要占到整本文件的 1/[`COMPACT_FREE_SHARE`] 才算数。
/// 只过大小的门还不够 —— 一本几乎满的库里那 1 MB 空洞还远不值得整本搬一次。
pub const COMPACT_FREE_SHARE: u64 = 4;

const CREATE: &str = "
CREATE TABLE IF NOT EXISTS captures (
  id INTEGER PRIMARY KEY,
  captured_at INTEGER NOT NULL,
  monitor_id TEXT NOT NULL DEFAULT '',
  x INTEGER NOT NULL,
  y INTEGER NOT NULL,
  w INTEGER NOT NULL,
  h INTEGER NOT NULL,
  source_kind TEXT NOT NULL,
  path TEXT,
  thumb BLOB,
  cancelled INTEGER NOT NULL DEFAULT 0,
  locked INTEGER NOT NULL DEFAULT 0,
  ocr_text TEXT,
  note TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS captures_captured_at ON captures(captured_at);
CREATE INDEX IF NOT EXISTS captures_locked ON captures(locked);
";

const COLUMNS: &str = "id, captured_at, monitor_id, x, y, w, h, source_kind, path, \
                       thumb, cancelled, locked, ocr_text, note";

pub struct History {
    conn: Connection,
    dir: PathBuf,
}

impl History {
    /// Open — creating it if this is the first run — `dir/history.db`. Safe on
    /// every start: the schema is `IF NOT EXISTS`, and the version check only
    /// refuses a *newer* file rather than rewriting an older one.
    pub fn open(dir: &Path) -> Result<Self, HistoryError> {
        std::fs::create_dir_all(dir)?;
        let conn = Connection::open(dir.join(DB_FILE))?;
        // A capture thread writes while a history page reads.
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(HistoryError::Schema(version));
        }
        if version == 0 {
            conn.execute_batch(CREATE)?;
            conn.execute(&format!("PRAGMA user_version = {SCHEMA_VERSION}"), [])?;
        }
        Ok(Self {
            conn,
            dir: dir.to_path_buf(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Write one capture: the row first, then the picture, then the row's
    /// `path`. The row has to exist before the file can be named after its id, so
    /// the whole thing is one transaction, and a failed file write takes the row
    /// back with it — an entry that points at nothing is worse than no entry.
    pub fn record(
        &mut self,
        meta: &CaptureMeta<'_>,
        frame: &Frame,
        cfg: &Settings,
    ) -> Result<i64, HistoryError> {
        let thumb = if cfg.thumb_px == 0 || cfg.thumb_px > 4096 {
            None
        } else {
            Some(thumbnail(frame, cfg.thumb_px)?)
        };

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO captures (captured_at, monitor_id, x, y, w, h, source_kind, \
                                    thumb, cancelled) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                meta.at_ms,
                meta.monitor_id,
                meta.rect.x,
                meta.rect.y,
                meta.rect.w as i64,
                meta.rect.h as i64,
                meta.source.as_str(),
                thumb,
                meta.cancelled,
            ],
        )?;
        let id = tx.last_insert_rowid();

        if cfg.keep_originals {
            let rel = relative_path(meta, id);
            let abs = self.dir.join(&rel);
            std::fs::create_dir_all(abs.parent().unwrap_or(self.dir.as_ref()))?;
            let written = encode::save(
                frame,
                &abs,
                &encode::EncodeOptions {
                    format: Format::Png,
                    quality: 92,
                    flatten_on_lossy: true,
                },
            );
            match written {
                Ok(()) => {
                    tx.execute(
                        "UPDATE captures SET path = ?1 WHERE id = ?2",
                        params![rel, id],
                    )?;
                }
                Err(e) => {
                    drop(tx);
                    let _ = std::fs::remove_file(&abs);
                    return Err(e.into());
                }
            }
        }
        tx.commit()?;
        Ok(id)
    }

    pub fn get(&self, id: i64) -> Result<Option<Record>, HistoryError> {
        let sql = format!("SELECT {COLUMNS} FROM captures WHERE id = ?1");
        Ok(self
            .conn
            .query_row(&sql, params![id], row_to_record)
            .optional()?)
    }

    /// The newest `limit` records: the first page of a virtual list (PRD §6.2
    /// "paging or a virtual list").
    pub fn newest(&self, limit: usize) -> Result<Vec<Record>, HistoryError> {
        self.page(None, limit, false)
    }

    /// What comes after `after` when the list scrolls down. The cursor is the
    /// record itself, not its id: two captures can share a `captured_at`, and the
    /// pair `(captured_at, id)` is what the ordering is defined on.
    pub fn older_than(&self, after: &Record, limit: usize) -> Result<Vec<Record>, HistoryError> {
        self.page(Some(after), limit, false)
    }

    /// PRD §5.14.3 步 3「找到标记为“已取消”的记录」里**筛**那一半的对面：与上面两条读腿同一条
    /// SELECT、同一副 `(captured_at, id)` 倒序，只多一枚 `cancelled = 1`。`after` 为 `None`
    /// 是第一页，为 `Some(游标)` 就是比那一行更旧的已取消记录。
    ///
    /// **筛住在查询里，不住在读完之后**：§7.1 分页那一支的两句话（「一页比页宽短」与「这一页
    /// 是空的」）说的都是「库里再没有更旧的行了」。读完再自己截掉不匹配的，短的那一页就成了
    /// 一句猜 —— 屏上会在还有更旧的已取消记录时写「到底了」。LIMIT 数的是**筛过之后**的行，
    /// 这一句才当场作数。
    pub fn cancelled_page(
        &self,
        after: Option<&Record>,
        limit: usize,
    ) -> Result<Vec<Record>, HistoryError> {
        self.page(after, limit, true)
    }

    /// 清单那三条读腿**唯一**的一条 SELECT：游标可有无、`cancelled` 可筛，两枚子句在这里拼，
    /// 所以次序与筛法只有一位作者（#281）—— 三家各写一份 `ORDER BY`，改一天次序就只改得了一处。
    fn page(
        &self,
        after: Option<&Record>,
        limit: usize,
        cancelled_only: bool,
    ) -> Result<Vec<Record>, HistoryError> {
        let flagged = if cancelled_only {
            " WHERE cancelled = 1"
        } else {
            ""
        };
        match after {
            None => self.collect(
                &format!(
                    "SELECT {COLUMNS} FROM captures{flagged} \
                     ORDER BY captured_at DESC, id DESC LIMIT ?1"
                ),
                params![limit as i64],
            ),
            Some(was) => {
                // 游标那一枚条件前面已经有筛子时接 `AND`，没有筛子时它自己就是 `WHERE`。
                let conj = if cancelled_only { " AND" } else { " WHERE" };
                self.collect(
                    &format!(
                        "SELECT {COLUMNS} FROM captures{flagged}{conj} (captured_at, id) < (?1, ?2) \
                         ORDER BY captured_at DESC, id DESC LIMIT ?3"
                    ),
                    params![was.captured_at, was.id, limit as i64],
                )
            }
        }
    }

    /// What comes before `before` when the list scrolls up.
    pub fn newer_than(&self, before: &Record, limit: usize) -> Result<Vec<Record>, HistoryError> {
        self.collect(
            &format!(
                "SELECT {COLUMNS} FROM captures WHERE (captured_at, id) > (?1, ?2) \
                 ORDER BY captured_at ASC, id ASC LIMIT ?3"
            ),
            params![before.captured_at, before.id, limit as i64],
        )
    }

    /// 上一条 (§5.14.2) — the next record older than `id`.
    pub fn prev(&self, id: i64) -> Result<Option<Record>, HistoryError> {
        self.neighbour(id, "<", "DESC")
    }

    /// 下一条 (§5.14.2) — the next record newer than `id`.
    pub fn next(&self, id: i64) -> Result<Option<Record>, HistoryError> {
        self.neighbour(id, ">", "ASC")
    }

    /// One row away in capture order. If `id` is gone the inner select yields no
    /// row, the comparison becomes NULL, and that reads back as `None` rather
    /// than an error — the shortcut keys should not fail because a record was
    /// cleaned up while the page was open.
    fn neighbour(&self, id: i64, dir: &str, order: &str) -> Result<Option<Record>, HistoryError> {
        let sql = format!(
            "SELECT {COLUMNS} FROM captures WHERE (captured_at, id) {dir} \
             (SELECT captured_at, id FROM captures WHERE id = ?1) \
             ORDER BY captured_at {order}, id {order} LIMIT 1"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map(params![id], row_to_record)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn count(&self) -> Result<u64, HistoryError> {
        let n = self
            .conn
            .query_row("SELECT COUNT(*) FROM captures", [], |r| r.get::<_, i64>(0))?;
        Ok(n as u64)
    }

    pub fn set_locked(&self, id: i64, locked: bool) -> Result<(), HistoryError> {
        self.update("locked", id, params![locked, id])
    }

    /// §5.12: the text an OCR run found, kept with the picture it came from.
    pub fn set_ocr(&self, id: i64, text: Option<&str>) -> Result<(), HistoryError> {
        self.update("ocr_text", id, params![text, id])
    }

    fn update(
        &self,
        column: &str,
        id: i64,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<(), HistoryError> {
        let n = self.conn.execute(
            &format!("UPDATE captures SET {column} = ?1 WHERE id = ?2"),
            params,
        )?;
        if n == 0 {
            return Err(HistoryError::Missing(id));
        }
        Ok(())
    }

    /// Manual deletion (PRD §5.14.4 steps 2–4). The lock is the user's own
    /// choice here, which is why this is not the same call as [`Self::cleanup`].
    pub fn delete(&self, ids: &[i64]) -> Result<Freed, HistoryError> {
        let mut out = Freed::default();
        for id in ids {
            let Some(rec) = self.get(*id)? else { continue };
            out.bytes += self.bytes_of(&rec);
            self.unlink(&rec)?;
            self.conn
                .execute("DELETE FROM captures WHERE id = ?1", params![id])?;
            out.ids.push(*id);
        }
        if !out.ids.is_empty() {
            self.prune_days();
            out.compacted = Some(self.compact());
        }
        Ok(out)
    }

    /// "Clear history". The 二次确认 belongs to the UI (§5.14.4); this only
    /// declines to touch locked records unless told otherwise.
    pub fn clear_all(&self, include_locked: bool) -> Result<Freed, HistoryError> {
        let sql = if include_locked {
            "SELECT id FROM captures"
        } else {
            "SELECT id FROM captures WHERE locked = 0"
        };
        let ids = self.ids(sql)?;
        self.delete(&ids)
    }

    /// §6.2 存储规则「临时文件应按策略自动清理」与 §8.7「历史记录空间不足」里**那枚文件
    /// 自己**那一半：`delete` / `clear_all` / `cleanup` 交回行与那些 PNG 之后，SQLite 只是
    /// 把页挂进 freelist，`history.db` 一寸不让 —— 全仓此前读不到 `VACUUM`（`open` 那一句
    /// `journal_mode=WAL` 是唯一一枚 pragma），所以删完之后盘上那本库还是删之前那么大。
    /// 本方法就是把那些空页真的还回去。
    ///
    /// **为什么是"问过才动手"**：VACUUM 的代价按重写的那本文件算，一次删一行就整本搬的话，
    /// §5.14.4 那一步用户等的那一下就从"改一行"变成"搬一本几十 MB 的库"。所以门读两枚当场
    /// 的 pragma（`page_count` / `freelist_count`，#455：不数条目、不猜页数），要同时过两条：
    /// 空洞字节 ≥ [`COMPACT_MIN_FREE_BYTES`]，且空洞 ≥ 整本的 1/[`COMPACT_FREE_SHARE`]。
    /// 门只有这一位作者（#281）：调用点一律问"这一问的答案是什么"，谁也不自己再判一次
    /// 值不值得 —— 三条删除腿（手动删、清空、自动清理）都从 [`Self::delete`] 与
    /// [`Self::cleanup`] 那两处过这一步，"什么时候压实"因此只写在 core。
    ///
    /// **为什么不是 `PRAGMA auto_vacuum=INCREMENTAL`**：改那一枚本身就得整本 VACUUM 一次
    /// 才生效，而它把归还空洞的代价摊到**每一次写**上面（删一行顺手截一次文件），比在这里
    /// 攒够了再搬一次贵得多，何况现有用户的库已经躺在 `auto_vacuum=NONE` 上。
    ///
    /// **WAL（当场验过两回，不是想当然）**：`open` 起的是 WAL，所以两边都不肯说现成的话 ——
    /// VACUUM 写回来的那本新库先进 `-wal`，主文件要等一次 checkpoint 才真的变小；而反方向
    /// 更阴：写进去的那本旧库此刻也还挂在 `-wal` 上，`usage` 那一格读的却是
    /// `std::fs::metadata(history.db)`，于是它报的是一个**还没落地的、偏小**的主文件。所以
    /// 这一步前后各一枚 `PRAGMA wal_checkpoint(TRUNCATE)`：前一枚把主文件落到真话，`before`
    /// 才是用户看得见的那个数（不设这一手，量出来的 `reclaimed` 会是 0 甚至是负的 —— 单测里
    /// 第一版就这么栽过一次）；后一枚把 VACUUM 那本新库写回主文件并把 `-wal` 截成零字节，
    /// 不留一份比库还胖的 WAL 让下一个读者去猜，`-shm` 随最后一把连接关闭自己退场。
    /// `Skipped` 那一臂不 checkpoint：那是 SQLite 本来就会自己做的两件事（写满 1000 页、
    /// 最后一把连接关闭），在这里替它多做一次是白花一趟写。
    ///
    /// 这一枚 pragma **不报错也不等**：有别的读事务在场（产品里就是同时开着的那张历史页）它
    /// 答 `busy=1`，此时主文件还没落地，本方法把这一臂说成 [`Compacted::Refused`] 而不是
    /// "压实成功"，因为用户此刻去看的字节数确实没变。
    ///
    /// **不抛错**：删除已经成了。这一步失败不能把它读成一次失败的删除（§5.14.4 那一步的
    /// 判据是行与那些文件已经没了），原因走 [`Compacted::Refused`]，由调用点本来就在说的
    /// 那一句（`mask.rs` 的 `history_delete_last` / `history_clear_last` / `history_last`）
    /// 交回用户 —— 既不静默，也不冒领一次失败。
    fn compact(&self) -> Compacted {
        let (pages, free, page_bytes) = match self.db_layout() {
            Ok(layout) => layout,
            Err(e) => return Compacted::Refused { reason: e },
        };
        let free_bytes = free.saturating_mul(page_bytes);
        if free_bytes < COMPACT_MIN_FREE_BYTES || free.saturating_mul(COMPACT_FREE_SHARE) < pages {
            return Compacted::Skipped { free_bytes };
        }
        // 前后各问一次文件系统：`reclaimed` 说的是用户能在目录里看见的那几个字节，
        // 不是 pragma 报的那一串页 —— 所以先落一次地，那本主文件才配当被减数。
        let file = self.dir.join(DB_FILE);
        if let Err(reason) = self.truncate_wal() {
            return Compacted::Refused { reason };
        }
        let before = self.file_bytes(&file);
        if let Err(e) = self.conn.execute_batch("VACUUM;") {
            return Compacted::Refused {
                reason: e.to_string(),
            };
        }
        if let Err(reason) = self.truncate_wal() {
            return Compacted::Refused { reason };
        }
        Compacted::Done {
            free_bytes,
            reclaimed: before.saturating_sub(self.file_bytes(&file)),
        }
    }

    /// 门要的那三个数，全部当场问 pragma：整本多少页、其中多少页在 freelist、一页多少字节。
    /// 出错时交回一句能直接说给用户的话（本档不外抛 `HistoryError`，因为它只服务于
    /// [`Self::compact`] 那一格不冒领失败的答法）。
    fn db_layout(&self) -> Result<(u64, u64, u64), String> {
        let pages = self.pragma_u64("page_count")?;
        let free = self.pragma_u64("freelist_count")?;
        let page_bytes = self.pragma_u64("page_size")?;
        Ok((pages, free, page_bytes))
    }

    fn pragma_u64(&self, name: &str) -> Result<u64, String> {
        self.conn
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get::<_, i64>(0))
            .map(|v| v.max(0) as u64)
            .map_err(|e| format!("`PRAGMA {name}` 读不回：{e}"))
    }

    /// 把 WAL 写回主文件并截干，答 `busy` 的那一臂在这里变成 Err（值就是那句话）。
    /// 三列读数一并写进那句话：`busy`、`-wal` 当时的页数、真的落进主文件的页数 —— 用户那句
    /// "删完怎么没小"对面是这三个数，不是一个"成功了"。
    fn truncate_wal(&self) -> Result<(), String> {
        let (busy, log, copied): (i64, i64, i64) = self
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(|e| format!("`wal_checkpoint(TRUNCATE)` 读不回：{e}"))?;
        if busy != 0 {
            return Err(format!(
                "`wal_checkpoint(TRUNCATE)` 答 busy（-wal 里 {log} 页、落进主文件 {copied} 页）：\
                 有别的读事务在场，页还挂在 WAL 上没落地"
            ));
        }
        Ok(())
    }

    /// The cleaner (PRD §5.14.1 "record count, retention time and maximum disk
    /// usage" / §5.14.4). 计划 §6.2 says it compares all three and stops only
    /// when all three pass. This does it in one newest-first pass rather than a
    /// delete-one-requery loop: age is decided per record, while the count and
    /// the byte budget are a prefix — once the list is full or the budget spent,
    /// everything older goes with it. Keeping them a prefix is also what stops
    /// the cleaner from trading a newer capture for an older one to fit the
    /// budget, which the loop version would happily do.
    ///
    /// Locked records stay when `exempt_locked`, so the result may still be over
    /// `max_items` or over budget: a spared record is not a candidate, but it
    /// does occupy a place in the list and on the disk, and is counted for both.
    pub fn cleanup(&self, policy: &Retention) -> Result<Freed, HistoryError> {
        struct Row {
            id: i64,
            captured_at: i64,
            locked: bool,
            path: Option<String>,
            bytes: u64,
        }
        let mut stmt = self.conn.prepare(
            "SELECT id, captured_at, locked, path, COALESCE(LENGTH(thumb), 0) \
             FROM captures ORDER BY captured_at DESC, id DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    captured_at: r.get(1)?,
                    locked: r.get(2)?,
                    path: r.get(3)?,
                    bytes: r.get::<_, i64>(4)? as u64,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);

        let now = now_ms();
        let mut kept_count: usize = 0;
        let mut kept_bytes: u64 = 0;
        let mut truncated = false;
        let mut victims: Vec<(i64, Option<String>, u64)> = Vec::new();
        for row in rows {
            let mut size = row.bytes;
            if let Some(rel) = &row.path {
                size += self.file_bytes(&self.dir.join(rel));
            }
            let too_old = policy.max_age_ms > 0 && now - row.captured_at > policy.max_age_ms;
            truncated |= policy.max_items > 0 && kept_count >= policy.max_items;
            truncated |= policy.max_bytes > 0 && kept_bytes.saturating_add(size) > policy.max_bytes;
            if too_old || truncated {
                if row.locked && policy.exempt_locked {
                    kept_count += 1;
                    kept_bytes = kept_bytes.saturating_add(size);
                    continue;
                }
                victims.push((row.id, row.path, size));
                continue;
            }
            kept_count += 1;
            kept_bytes = kept_bytes.saturating_add(size);
        }

        // The walk is newest-first; what went, went oldest-first, and that is the
        // order a caller wants to report ("dropped 12 captures from 3 Oct up").
        let mut out = Freed::default();
        for (id, path, size) in victims.into_iter().rev() {
            self.conn
                .execute("DELETE FROM captures WHERE id = ?1", params![id])?;
            if let Some(rel) = &path {
                self.remove(&self.dir.join(rel))?;
            }
            out.bytes += size;
            out.ids.push(id);
        }
        if !out.ids.is_empty() {
            self.prune_days();
            // §6.2 那句"按策略自动清理"清的也是这本文件：策略交走的行越多，freelist 越长，
            // 而这一腿跑在每次记成之后（`mask.rs:2490`），所以值不值得搬整本库由
            // [`Self::compact`] 那一扇门说，不由这里再判一次（#281）。
            out.compacted = Some(self.compact());
        }
        Ok(out)
    }

    /// PRD §6.2: the settings page shows what the history occupies. Originals
    /// count from the filesystem, so a record whose file went missing contributes
    /// its thumbnail only — the honest number, not the one the row claims.
    pub fn usage(&self) -> Result<Usage, HistoryError> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, COALESCE(LENGTH(thumb), 0) FROM captures")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)? as u64))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);

        let mut out = Usage {
            records: rows.len() as u64,
            db_bytes: self.file_bytes(&self.dir.join(DB_FILE)),
            ..Default::default()
        };
        for (path, thumb) in rows {
            out.thumb_bytes += thumb;
            if let Some(rel) = path {
                out.original_bytes += self.file_bytes(&self.dir.join(rel));
            }
        }
        Ok(out)
    }

    /// The picture a record holds: the original while it is on disk, the
    /// thumbnail after it is gone. §5.14.2 offers 重新复制 / 保存 / 标注 / 贴图
    /// from either.
    pub fn frame(&self, rec: &Record) -> Result<Frame, HistoryError> {
        if let Some(abs) = self.path_of(rec) {
            if abs.exists() {
                return Ok(encode::decode_file(&abs)?);
            }
        }
        match rec.thumb.as_ref() {
            Some(bytes) => Ok(encode::decode_as(bytes, Some(Format::Jpg))?),
            None => Err(HistoryError::Missing(rec.id)),
        }
    }

    /// Where a record's original lives, resolved against this store's directory.
    /// Not recomputed from the id, because a record can outlive the dated-folder
    /// rule that made it, and its stored path is the only truth about where it
    /// went.
    pub fn path_of(&self, rec: &Record) -> Option<PathBuf> {
        rec.path.as_ref().map(|rel| self.dir.join(rel))
    }

    fn bytes_of(&self, rec: &Record) -> u64 {
        let thumb = rec.thumb.as_ref().map_or(0, |v| v.len() as u64);
        match self.path_of(rec) {
            Some(p) => thumb + self.file_bytes(&p),
            None => thumb,
        }
    }

    fn file_bytes(&self, path: &Path) -> u64 {
        std::fs::metadata(path).map(|m| m.len()).unwrap_or_default()
    }

    fn remove(&self, path: &Path) -> Result<(), HistoryError> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            // Already gone is already freed.
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn unlink(&self, rec: &Record) -> Result<(), HistoryError> {
        if let Some(p) = self.path_of(rec) {
            self.remove(&p)?;
        }
        Ok(())
    }

    /// Dated folders left empty by a delete. §5.14.4 promises the space back; a
    /// folder of nothing looks like history to the next person who opens the
    /// directory. A failure here is not worth reporting.
    fn prune_days(&self) {
        let Ok(entries) = std::fs::read_dir(self.dir.join(PICTURES_DIR)) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let empty = std::fs::read_dir(&path)
                .map(|mut d| d.next().is_none())
                .unwrap_or(false);
            if empty {
                let _ = std::fs::remove_dir(path);
            }
        }
    }

    fn ids(&self, sql: &str) -> Result<Vec<i64>, HistoryError> {
        let mut stmt = self.conn.prepare(sql)?;
        let out = stmt
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }

    fn collect(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<Record>, HistoryError> {
        let mut stmt = self.conn.prepare(sql)?;
        let out = stmt
            .query_map(params, row_to_record)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(out)
    }
}

/// `history/<YYYY-MM-DD>/<id>.png`, the layout §6.2 fixes. Slashes go into the
/// database even on Windows: the path is data, and a folder that moves to another
/// machine has to keep reading.
fn relative_path(meta: &CaptureMeta<'_>, id: i64) -> String {
    let stamp = LocalStamp::from_epoch(meta.at_ms / 1000, meta.offset_min);
    format!(
        "{PICTURES_DIR}/{:04}-{:02}-{:02}/{id}.png",
        stamp.year, stamp.month, stamp.day
    )
}

/// Downscale to fit a square of `px` edge, keeping the aspect ratio, and JPEG it.
/// [`Frame::resized`] answers the size it is asked for, so the proportion is
/// worked out here rather than left to the codec.
pub fn thumbnail(frame: &Frame, px: u32) -> Result<Vec<u8>, crate::encode::EncodeError> {
    let long = frame.width.max(frame.height).max(1);
    let (w, h) = if long <= px {
        (frame.width, frame.height)
    } else {
        let s = px as f64 / long as f64;
        (
            ((frame.width as f64 * s).round().max(1.0)) as u32,
            ((frame.height as f64 * s).round().max(1.0)) as u32,
        )
    };
    let small = if w == frame.width && h == frame.height {
        std::borrow::Cow::Borrowed(frame)
    } else {
        std::borrow::Cow::Owned(frame.resized(w, h, true)?)
    };
    encode::encode(
        &small,
        &encode::EncodeOptions {
            format: Format::Jpg,
            quality: THUMB_QUALITY,
            flatten_on_lossy: true,
        },
    )
}

/// §5.14.2 step 4's 重新标注 and §5.14.3 step 4's 恢复内容 need one more thing
/// than a decoder: the row's own picture has to be sitting **where the row says it
/// was** inside a freshly frozen desktop, because the mask textures every screen
/// from that snapshot and the annotation layer takes its base from the same place.
/// This function is where that claim gets made, once, in the pure layer: the
/// bridge stamps and opens, and neither side re-decides which pixels go where.
///
/// Three rules the callers should not have to repeat:
///
/// * The picture is the row's, never the desktop's. A row whose stored file is
///   gone falls back to its thumbnail through [`History::frame`], and this
///   function resizes whatever arrives to the record's own rect — the frame on
///   screen and the selection around it are then two readings of one number
///   rather than a 128px thumbnail inside an 800px hole.
/// * A rect that spans a seam is painted on **both** monitors, each getting the
///   slice that overlaps it. Handing the whole picture to [`Frame::paste`] per
///   monitor is what makes that free: `paste` clips to the destination, so the
///   off-monitor edge is dropped rather than wrapped.
/// * Nothing is invented for a row that cannot land. Both refusals are answers the
///   caller has to be able to tell apart (see [`HistoryError`]'s two new arms);
///   a partially covered rect is *not* a refusal — painting what fits is exactly
///   what the user asked for, and the clipped-off part was never on this desktop.
///
/// Returns how many monitors carry the row's pixels afterwards, so a caller that
/// wants to say "two screens" can say it from this number rather than counting.
pub fn stamp_row(
    snap: &mut ScreenSnapshot,
    rect: &PhysRect,
    pic: &Frame,
) -> Result<u32, HistoryError> {
    if rect.is_empty() {
        return Err(HistoryError::EmptyRect(*rect));
    }
    // 尺寸不合就按那一行的矩形重画。缩略图（`keep_originals` 关着时 `frame` 的返回值）
    // 与原始像素的比例一致，所以这一步不会改变"看到的是哪张图"，只改变它有多大。
    let whole: std::borrow::Cow<'_, Frame> = if pic.width == rect.w && pic.height == rect.h {
        std::borrow::Cow::Borrowed(pic)
    } else {
        std::borrow::Cow::Owned(pic.resized(rect.w, rect.h, true)?)
    };
    let mut touched = 0u32;
    for m in &mut snap.monitors {
        // The row's own corner, told in *that monitor's* pixels. It may be
        // negative — a rect that straddles a seam starts to the left of the
        // screen on the right — and that is exactly what [`Frame::paste`] wants:
        // it clips the destination and reads the picture from the matching
        // column, so each screen gets its own slice of one picture instead of
        // two screens each getting the whole thing (or, worse, the left slice of
        // it painted onto the right screen's left edge).
        if m.info.bounds.intersection(rect).is_none() {
            continue;
        }
        let at = PhysPoint::new(rect.x - m.info.bounds.x, rect.y - m.info.bounds.y);
        if m.frame.paste(&whole, at).is_some() {
            touched += 1;
        }
    }
    if touched == 0 {
        return Err(HistoryError::OffSnapshot(*rect));
    }
    Ok(touched)
}

fn row_to_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<Record> {
    Ok(Record {
        id: r.get(0)?,
        captured_at: r.get(1)?,
        monitor_id: r.get(2)?,
        rect: PhysRect {
            x: r.get(3)?,
            y: r.get(4)?,
            w: r.get::<_, i64>(5)? as u32,
            h: r.get::<_, i64>(6)? as u32,
        },
        source: Source::parse(&r.get::<_, String>(7)?).unwrap_or(Source::Region),
        path: r.get(8)?,
        thumb: r.get(9)?,
        cancelled: r.get(10)?,
        locked: r.get(11)?,
        ocr_text: r.get(12)?,
        note: r.get(13)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{FrozenMonitor, MonitorInfo};
    use crate::geometry::Scale;
    use proptest::prelude::*;

    fn frame(w: u32, h: u32, rgba: [u8; 4]) -> Frame {
        Frame::filled(w, h, rgba).unwrap()
    }

    fn settings() -> Settings {
        Settings::default()
    }

    fn at(ms: i64) -> CaptureMeta<'static> {
        CaptureMeta {
            at_ms: ms,
            offset_min: 0,
            monitor_id: "mon-0",
            rect: PhysRect {
                x: 10,
                y: 20,
                w: 30,
                h: 40,
            },
            source: Source::Region,
            cancelled: false,
        }
    }

    /// No limit set. Written out instead of derived so a policy in a test shows
    /// every condition it means to leave off.
    fn unset() -> Retention {
        Retention {
            max_items: 0,
            max_age_ms: 0,
            max_bytes: 0,
            exempt_locked: false,
        }
    }

    struct Store {
        // The temporary directory outliving the handle is the whole point of the
        // struct; nothing reads it.
        _dir: tempfile::TempDir,
        h: History,
    }

    fn store() -> Store {
        let dir = tempfile::tempdir().unwrap();
        let h = History::open(dir.path()).unwrap();
        Store { _dir: dir, h }
    }

    #[test]
    fn opening_the_same_directory_twice_keeps_every_record() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut h = History::open(dir.path()).unwrap();
            h.record(&at(1_000), &frame(8, 8, [1, 2, 3, 255]), &settings())
                .unwrap();
        }
        let h = History::open(dir.path()).unwrap();
        assert_eq!(h.count().unwrap(), 1);
    }

    #[test]
    fn a_record_comes_back_with_its_geometry_and_source() {
        let mut t = store();
        let mut meta = at(123_456);
        meta.source = Source::Window;
        meta.monitor_id = "mon-1";
        let id =
            t.h.record(&meta, &frame(16, 9, [9, 9, 9, 255]), &settings())
                .unwrap();
        let rec = t.h.get(id).unwrap().unwrap();
        assert_eq!(rec.captured_at, 123_456);
        assert_eq!(rec.monitor_id, "mon-1");
        assert_eq!(rec.rect, meta.rect);
        assert_eq!(rec.source, Source::Window);
        assert!(!rec.cancelled);
        assert!(!rec.locked);
        assert_eq!(rec.note, "");
        assert_eq!(rec.ocr_text, None);
    }

    /// An instant chosen so that UTC and UTC+2 fall on different days: 22:33 on
    /// 5 Oct UTC is 00:33 on 6 Oct two hours east.
    const NEAR_MIDNIGHT_S: i64 = 1_759_703_600;

    #[test]
    fn the_original_lands_in_a_dated_folder_the_row_points_at() {
        let mut t = store();
        let mut meta = at(NEAR_MIDNIGHT_S * 1_000);
        meta.offset_min = 120;
        let id =
            t.h.record(&meta, &frame(12, 12, [1, 2, 3, 255]), &settings())
                .unwrap();
        let rec = t.h.get(id).unwrap().unwrap();
        let rel = rec.path.clone().expect("keep_originals is on");
        assert_eq!(rel, relative_path(&meta, id), "the §6.2 layout");
        assert!(rel.starts_with("history/2025-10-06/"), "{rel}");
        assert_ne!(
            rel,
            relative_path(&at(NEAR_MIDNIGHT_S * 1_000), id),
            "the same instant filed by UTC would have been the day before"
        );
        let abs = t.h.path_of(&rec).unwrap();
        assert!(abs.exists(), "{abs:?} was written");
        assert!(abs.starts_with(t.h.dir()));
        assert_eq!(
            encode::decode_file(&abs).unwrap().pixels,
            frame(12, 12, [1, 2, 3, 255]).pixels,
            "PNG is lossless, so the history gives the capture back exactly"
        );
    }

    #[test]
    fn the_dated_folder_follows_the_local_clock_not_utc() {
        let utc = LocalStamp::from_epoch(NEAR_MIDNIGHT_S, 0);
        let east = LocalStamp::from_epoch(NEAR_MIDNIGHT_S, 120);
        assert_eq!((utc.year, utc.month, utc.day, utc.hour), (2025, 10, 5, 22));
        assert_eq!(
            (east.year, east.month, east.day),
            (2025, 10, 6),
            "check the fixture"
        );
    }

    #[test]
    fn the_thumbnail_is_a_smaller_picture_of_the_picture() {
        let mut t = store();
        let id =
            t.h.record(
                &at(1),
                &frame(600, 300, [200, 100, 50, 255]),
                &Settings {
                    thumb_px: 100,
                    ..settings()
                },
            )
            .unwrap();
        let rec = t.h.get(id).unwrap().unwrap();
        let bytes = rec.thumb.as_ref().unwrap();
        let thumb = encode::decode_as(bytes, Some(Format::Jpg)).unwrap();
        assert_eq!((thumb.width, thumb.height), (100, 50), "aspect kept");
        assert!(
            (bytes.len() as u64) < t.h.file_bytes(&t.h.path_of(&rec).unwrap()),
            "the point of a thumbnail is that it is the smaller copy"
        );
    }

    #[test]
    fn a_small_picture_keeps_its_pixels_in_the_thumbnail() {
        let bytes = thumbnail(&frame(8, 6, [0, 0, 0, 255]), 256).unwrap();
        let back = encode::decode_as(&bytes, Some(Format::Jpg)).unwrap();
        assert_eq!((back.width, back.height), (8, 6), "never upscaled");
    }

    #[test]
    fn a_cancelled_capture_is_flagged_rather_than_left_out() {
        let mut t = store();
        let mut meta = at(5);
        meta.cancelled = true;
        let id =
            t.h.record(&meta, &frame(4, 4, [0, 0, 0, 255]), &settings())
                .unwrap();
        // §5.14.3: the user finds it in the list and takes the content back.
        let rec = t.h.get(id).unwrap().unwrap();
        assert!(rec.cancelled);
        assert_eq!(t.h.frame(&rec).unwrap().width, 4);
        assert_eq!(t.h.count().unwrap(), 1);
    }

    #[test]
    fn without_keep_originals_the_record_is_its_thumbnail() {
        let mut t = store();
        let id =
            t.h.record(
                &at(7),
                &frame(64, 64, [3, 4, 5, 255]),
                &Settings {
                    keep_originals: false,
                    ..settings()
                },
            )
            .unwrap();
        let rec = t.h.get(id).unwrap().unwrap();
        assert_eq!(rec.path, None);
        assert!(!t.h.dir().join(PICTURES_DIR).exists());
        assert_eq!(t.h.frame(&rec).unwrap().width, 64, "still usable");
    }

    #[test]
    fn the_list_is_newest_first_and_pages_off_a_cursor() {
        let mut t = store();
        let mut ids = Vec::new();
        for i in 0..6i64 {
            ids.push(
                t.h.record(&at(i * 1_000), &frame(4, 4, [i as u8; 4]), &settings())
                    .unwrap(),
            );
        }
        let page1 = t.h.newest(4).unwrap();
        let page2 = t.h.older_than(&page1[3], 4).unwrap();
        let all: Vec<i64> = page1.iter().chain(page2.iter()).map(|r| r.id).collect();
        assert_eq!(
            all,
            vec![ids[5], ids[4], ids[3], ids[2], ids[1], ids[0]],
            "one page ends where the next begins"
        );
        assert!(t.h.older_than(&page2[1], 4).unwrap().is_empty());

        // 上一条 / 下一条 (§5.14.2) is the same walk, one step at a time.
        assert_eq!(t.h.prev(ids[3]).unwrap().unwrap().id, ids[2]);
        assert_eq!(t.h.next(ids[3]).unwrap().unwrap().id, ids[4]);
        assert!(t.h.prev(ids[0]).unwrap().is_none());
        assert!(t.h.next(ids[5]).unwrap().is_none());
        assert!(
            t.h.prev(9999).unwrap().is_none(),
            "a gone id is not an error"
        );
    }

    /// PRD §5.14.3 步 3「找到标记为“已取消”的记录」里**筛**那一半的三臂：筛开着而库里有已取消
    /// 的行 ⇒ 只回那些行、`(captured_at, id)` 那一趟翻页照走；筛开着而库里没有 ⇒ `Ok(空)`，那是
    /// 一张筛过的空清单而不是「库里没行」，也不是错；筛关着 ⇒ 与这一格之前一字不差，没取消的那
    /// 几行一枚也不少。
    ///
    /// **短的那一页是判据而不是巧合**：`cancelled_page` 的 `LIMIT` 数的是筛过之后的行，所以
    /// 「这一页比页宽短」当场就等于「库里再没有更旧的已取消行」—— 屏上那句「清单到底了」
    /// （§7.1 分页那一支，`qt_bridge` 的 `MaskState::list_history_older`）靠的就是这一条。
    /// 读完再在界面那一头截掉不匹配的，短页就成了猜测，屏上会在还有更旧的已取消记录时说反话。
    #[test]
    fn the_cancelled_filter_lists_only_those_rows_and_still_runs_out() {
        let mut t = store();
        let mut ids = Vec::new();
        for i in 0..6i64 {
            let mut meta = at(i * 1_000);
            meta.cancelled = i % 2 == 0;
            ids.push(
                t.h.record(&meta, &frame(4, 4, [i as u8; 4]), &settings())
                    .unwrap(),
            );
        }
        let (zero, two, four) = (ids[0], ids[2], ids[4]);

        // ① 有得筛：第一页两枚（满的一页），第二页一枚（短的一页），第三页就该是空的。
        let first = t.h.cancelled_page(None, 2).unwrap();
        assert_eq!(
            first.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![four, two],
            "筛过的那一页混进了没取消的行，或次序不是 (captured_at, id) 倒序"
        );
        let second = t.h.cancelled_page(first.last(), 2).unwrap();
        assert_eq!(
            second.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![zero],
            "游标走的是筛过的那一行，不是库里那六行里的某一枚"
        );
        assert!(
            t.h.cancelled_page(second.last(), 2).unwrap().is_empty(),
            "已取消的那三行读完了，下一问就该是空的"
        );
        assert!(
            first.iter().chain(second.iter()).all(|r| r.cancelled),
            "筛出去的行里还有没取消的"
        );

        // ② 筛关着：同一座库、同一副次序，一枚没取消的行也不许被筛掉。
        let unfiltered = t.h.newest(10).unwrap();
        assert_eq!(unfiltered.len(), 6, "没筛的那一问不该少行");
        assert!(unfiltered.iter().any(|r| !r.cancelled));

        // ③ 无得可筛：库里三行全没取消 ⇒ 筛出来是空清单，而不是错误、也不是「库里没行」。
        let mut bare = store();
        for i in 0..3i64 {
            bare.h
                .record(&at(i * 1_000), &frame(4, 4, [9; 4]), &settings())
                .unwrap();
        }
        assert_eq!(bare.h.count().unwrap(), 3, "库里那三行没种成");
        assert!(
            bare.h.cancelled_page(None, 10).unwrap().is_empty(),
            "没有一行被取消过，筛出来的清单就该是空的"
        );
    }

    #[test]
    fn a_tie_on_the_clock_still_has_one_order() {
        let mut t = store();
        let a =
            t.h.record(&at(1_000), &frame(4, 4, [1; 4]), &settings())
                .unwrap();
        let b =
            t.h.record(&at(1_000), &frame(4, 4, [2; 4]), &settings())
                .unwrap();
        let list = t.h.newest(10).unwrap();
        assert_eq!(
            list.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![b, a],
            "newer id wins the tie, and paging cannot skip one"
        );
        assert_eq!(t.h.prev(b).unwrap().unwrap().id, a);
        assert_eq!(t.h.next(a).unwrap().unwrap().id, b);
    }

    #[test]
    fn ocr_and_lock_both_round_trip() {
        let mut t = store();
        let id =
            t.h.record(&at(1), &frame(4, 4, [0, 0, 0, 255]), &settings())
                .unwrap();
        t.h.set_ocr(id, Some("total 12.00")).unwrap();
        t.h.set_locked(id, true).unwrap();
        let rec = t.h.get(id).unwrap().unwrap();
        assert_eq!(rec.ocr_text.as_deref(), Some("total 12.00"));
        assert!(rec.locked);
        t.h.set_ocr(id, None).unwrap();
        assert_eq!(t.h.get(id).unwrap().unwrap().ocr_text, None);
        assert!(matches!(
            t.h.set_locked(9999, true),
            Err(HistoryError::Missing(9999))
        ));
    }

    #[test]
    fn deleting_a_record_takes_its_file_and_its_empty_day() {
        let mut t = store();
        let id =
            t.h.record(&at(1), &frame(64, 64, [1, 1, 1, 255]), &settings())
                .unwrap();
        let rec = t.h.get(id).unwrap().unwrap();
        let abs = t.h.path_of(&rec).unwrap();
        let before = t.h.file_bytes(&abs) + rec.thumb.as_ref().unwrap().len() as u64;
        let freed = t.h.delete(&[id]).unwrap();
        assert_eq!(freed.ids, vec![id]);
        assert_eq!(freed.bytes, before);
        assert!(!abs.exists(), "§5.14.4 step 4 releases the disk");
        assert!(!abs.parent().unwrap().exists(), "and the empty day with it");
        assert_eq!(t.h.count().unwrap(), 0);
        assert!(t.h.get(id).unwrap().is_none());
        assert_eq!(
            t.h.usage().unwrap().total_bytes(),
            t.h.usage().unwrap().db_bytes
        );
        // Deleting something already gone is not an error the UI must handle.
        assert!(t.h.delete(&[id]).unwrap().ids.is_empty());
    }

    /// 一把连接里的库要"删得出空洞"才谈得上压实：BLOB 是唯一住在那本文件里的大件，所以这里
    /// 喂的是 JPEG 压不动的噪声，并把 `keep_originals` 关掉（原图住在文件系统，与这里要量的
    /// 那枚 `history.db` 不相干）。行数不钉死 —— 循环到 `usage` 当场报回来的缩略图字节过得了
    /// [`COMPACT_MIN_FREE_BYTES`] 的两倍为止，门哪天改了这几格跟着改（#455）。
    fn fatten(h: &mut History) -> Vec<i64> {
        let cfg = Settings {
            keep_originals: false,
            thumb_px: 1024,
            ..Default::default()
        };
        let big = noise_frame(1024, 1024);
        let mut ids = Vec::new();
        while h.usage().unwrap().thumb_bytes < 2 * COMPACT_MIN_FREE_BYTES {
            ids.push(h.record(&at(((ids.len() + 1) * 1_000) as i64), &big, &cfg).unwrap());
        }
        ids
    }

    /// 确定性噪声，不为量具新添一枚 `rand` 依赖：一列 LCG 推出来的字节。
    fn noise_frame(w: u32, h: u32) -> Frame {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut pixels = vec![0u8; (w as usize) * (h as usize) * 4];
        for p in pixels.as_chunks_mut::<4>().0 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let b = (state >> 33) as u32;
            p.copy_from_slice(&[b as u8, (b >> 8) as u8, (b >> 16) as u8, 255]);
        }
        Frame::from_rgba(w, h, pixels).unwrap()
    }

    /// `-wal` 此刻在盘上占多少字节（没有那份文件就是 0）。压实跑完之后这一格必须是 0：否则
    /// "库小了"说的只是主文件，而那一本新的还整份挂在 WAL 上等下一个读者去落地。
    fn wal_bytes(dir: &Path) -> u64 {
        std::fs::metadata(dir.join(format!("{DB_FILE}-wal")))
            .map(|m| m.len())
            .unwrap_or_default()
    }

    /// §6.2 存储规则那句「临时文件应按策略自动清理」与 §5.14.4 步 4「释放相应磁盘空间」里
    /// 一直没成的那一半：**删掉行之后盘上那枚 `history.db` 自己也要跟着下来**。量的就是用户
    /// 看得见的那个数 —— [`History::usage`] 的 `db_bytes` 走 `std::fs::metadata(history.db)`，
    /// 而 `qt_bridge` 的 `settings.rs` 的 `history_usage_line` 把那一格原样排进给用户那五行里。
    ///
    /// 三问一起来：`Freed` 说的是那一格答了话（不是 `None`、也不是门挡下的 `Skipped`）、
    /// `Done` 报的字节数与用户后来从 `usage()` 看见的前后差是同一个数（不是另算一遍）、
    /// 以及 `-wal` 被截干了。
    ///
    /// 前置那一次「放下再开回来」不是讲究：产品里每条腿都自己开库、动一下、再放下
    /// （`settings.rs` 的 `history_open_in`），而放下那一次 SQLite 自己就把 WAL 落地了 ——
    /// 不换这一手，`before` 读到的是一份还没落地的旧主文件，量的就不是用户看得见的那个数
    /// （这一格第一版正是这样把 `reclaimed` 量成 0 的）。
    #[test]
    fn deleting_fat_rows_shrinks_the_database_file_itself() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = History::open(dir.path()).unwrap();
        let ids = fatten(&mut writer);
        drop(writer);
        let h = History::open(dir.path()).unwrap();
        let before = h.usage().unwrap();
        assert!(
            before.db_bytes > before.thumb_bytes / 2,
            "前置：那些 BLOB 确实住在这本文件里（主文件 {} 字节 vs BLOB {} 字节）",
            before.db_bytes,
            before.thumb_bytes
        );
        assert_eq!(wal_bytes(dir.path()), 0, "前置：放下过一次库，WAL 已经落地");
        let freed = h.delete(&ids).unwrap();
        assert_eq!(freed.ids, ids, "§5.14.4 点名的行一枚也没少");
        assert_eq!(h.count().unwrap(), 0);
        let free_before = match freed.compacted {
            Some(Compacted::Done {
                free_bytes,
                reclaimed,
            }) => {
                assert!(reclaimed > 0, "说压实了却一寸没缩");
                assert!(free_bytes >= COMPACT_MIN_FREE_BYTES, "门挡下的数被说成压实了");
                reclaimed
            }
            other => panic!("删空了这本库却没有压实这一格（`Freed::compacted`）: {other:?}"),
        };
        let after = h.usage().unwrap();
        assert!(
            after.db_bytes < before.db_bytes,
            "§6.2：删完行盘上的 history.db 还是 {} 字节（原 {}）",
            after.db_bytes,
            before.db_bytes
        );
        assert_eq!(
            free_before,
            before.db_bytes - after.db_bytes,
            "`Compacted::Done` 报的字节数与 `usage()` 那格必须是一个数，不是两处各算一遍"
        );
        assert_eq!(
            wal_bytes(dir.path()),
            0,
            "VACUUM 之后那本新库还整份挂在 -wal 上没落地"
        );
    }

    /// 门的另一臂，也是"一次寻常删除不许重写整本库"这句设计的证人：删掉很小的一行 ⇒
    /// [`Compacted::Skipped`]、文件一个字节也不动（`usage` 那格前后相等）。这条不许被
    /// 上那一格顺手证掉 —— 那边删的是全部行，这边删的是其中一行。
    #[test]
    fn an_ordinary_small_delete_leaves_the_database_file_alone() {
        let mut t = store();
        let small = frame(64, 64, [7, 9, 11, 255]);
        let a = t.h.record(&at(1), &small, &settings()).unwrap();
        let _b = t.h.record(&at(2), &small, &settings()).unwrap();
        let before = t.h.usage().unwrap().db_bytes;
        let freed = t.h.delete(&[a]).unwrap();
        assert_eq!(freed.ids, vec![a]);
        match freed.compacted {
            Some(Compacted::Skipped { free_bytes }) => assert!(
                free_bytes < COMPACT_MIN_FREE_BYTES,
                "这一次的空洞本来就小，却被记成过了门: {free_bytes} 字节"
            ),
            other => panic!("一次删一行的普通删除去搬整本库了: {other:?}"),
        }
        assert_eq!(
            t.h.usage().unwrap().db_bytes,
            before,
            "门没过 ⇒ 那本文件一个字节也不该动"
        );
    }

    /// §6.2「按策略自动清理」交走的行同样把那本文件腾空：`cleanup` 那一腿（产品读者
    /// `mask.rs` 每次记成之后跑的那一句）也走同一扇门、也说同一句话。上限取 `max_items`
    /// 一条，其余留空 —— 与 [`unset`] 同一个意思：写清楚这一格关掉的是哪几枚条件。
    #[test]
    fn the_policy_cleaner_shrinks_the_file_it_emptied() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = History::open(dir.path()).unwrap();
        let ids = fatten(&mut writer);
        drop(writer);
        let h = History::open(dir.path()).unwrap();
        let before = h.usage().unwrap().db_bytes;
        let policy = Retention {
            max_items: 1,
            max_age_ms: 0,
            max_bytes: 0,
            exempt_locked: false,
        };
        let freed = h.cleanup(&policy).unwrap();
        assert_eq!(freed.ids.len(), ids.len() - 1, "只该留下最新那一行");
        assert_eq!(h.count().unwrap(), 1);
        assert!(
            matches!(freed.compacted, Some(Compacted::Done { reclaimed, .. }) if reclaimed > 0),
            "策略清出来的空洞没被还回文件: {:?}",
            freed.compacted
        );
        assert!(
            h.usage().unwrap().db_bytes < before,
            "§6.2：自动清理跑完，盘上的 history.db 没跟着下来"
        );
        assert_eq!(wal_bytes(dir.path()), 0, "清理那一腿也没把 -wal 截干");
    }

    /// 压实失败不许把删除读成失败。堵手用的是产品里真会出现的那一手：另一把连接握住一个读
    /// 事务（同时开着的那张历史页就是它）—— WAL 允许它读、却不许此刻把 WAL 截干，于是
    /// `wal_checkpoint(TRUNCATE)` 当场答 `busy`。这时 `delete` 依旧交回 `Ok`、那些行依旧没了、
    /// `Freed::compacted` 说的是 [`Compacted::Refused`]，而那句话归 `mask.rs` 那三格 `*_last`
    /// 去说（那里本来就有拒绝自己说的地方，本条不新辟通道）。
    #[test]
    fn a_compaction_blocked_by_another_reader_still_delivers_the_delete() {
        let mut t = store();
        let ids = fatten(&mut t.h);
        let reader = Connection::open(t.h.dir().join(DB_FILE)).unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let held = reader
            .query_row("SELECT count(*) FROM captures", [], |r| r.get::<_, i64>(0))
            .unwrap();
        assert_eq!(held as usize, ids.len(), "前置：那把读事务确实读到了这些行");
        let freed = t.h.delete(&ids).unwrap();
        reader.execute_batch("COMMIT").unwrap();
        drop(reader);
        assert_eq!(freed.ids, ids, "别人堵住 WAL 不构成一次失败的删除");
        assert_eq!(t.h.count().unwrap(), 0, "那一行确实从库里没了");
        assert!(freed.bytes > 0, "`Freed::bytes` 那笔账不该被压实这一格改掉");
        match freed.compacted {
            Some(Compacted::Refused { reason }) => assert!(
                reason.contains("wal_checkpoint"),
                "压实失败说的话没交代是哪一步失败的: {reason}"
            ),
            other => panic!(
                "读事务在场时这一步本该答 busy，结果={other:?}（`-wal` 此刻 {} 字节）",
                wal_bytes(t.h.dir())
            ),
        }
    }

    #[test]
    fn a_day_folder_with_other_records_in_it_stays() {
        let mut t = store();
        let a =
            t.h.record(&at(1), &frame(8, 8, [1; 4]), &settings())
                .unwrap();
        let b =
            t.h.record(&at(2), &frame(8, 8, [2; 4]), &settings())
                .unwrap();
        let day =
            t.h.path_of(&t.h.get(a).unwrap().unwrap())
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf();
        t.h.delete(&[a]).unwrap();
        assert!(day.exists(), "only the record went, not its neighbour");
        assert!(t.h.path_of(&t.h.get(b).unwrap().unwrap()).unwrap().exists());
    }

    #[test]
    fn clearing_keeps_locked_records_until_told_otherwise() {
        let mut t = store();
        let a =
            t.h.record(&at(1), &frame(4, 4, [0, 0, 0, 255]), &settings())
                .unwrap();
        let b =
            t.h.record(&at(2), &frame(4, 4, [0, 0, 0, 255]), &settings())
                .unwrap();
        t.h.set_locked(b, true).unwrap();
        assert_eq!(t.h.clear_all(false).unwrap().ids, vec![a]);
        assert_eq!(t.h.count().unwrap(), 1);
        assert_eq!(t.h.clear_all(true).unwrap().ids, vec![b]);
        assert_eq!(t.h.count().unwrap(), 0);
    }

    #[test]
    fn the_count_limit_keeps_the_newest_and_spares_the_locked() {
        let mut t = store();
        let mut ids = Vec::new();
        for i in 0..5i64 {
            ids.push(
                t.h.record(&at(i * 1_000), &frame(4, 4, [i as u8; 4]), &settings())
                    .unwrap(),
            );
        }
        let freed =
            t.h.cleanup(&Retention {
                max_items: 3,
                ..unset()
            })
            .unwrap();
        assert_eq!(freed.ids, vec![ids[0], ids[1]], "the oldest go first");

        for i in 0..4i64 {
            ids.push(
                t.h.record(
                    &at(10_000 + i * 1_000),
                    &frame(4, 4, [i as u8; 4]),
                    &settings(),
                )
                .unwrap(),
            );
        }
        // ids[2] is now the oldest thing left in the list.
        t.h.set_locked(ids[2], true).unwrap();
        let freed =
            t.h.cleanup(&Retention {
                max_items: 2,
                exempt_locked: true,
                ..unset()
            })
            .unwrap();
        assert!(!freed.ids.contains(&ids[2]));
        assert_eq!(t.h.count().unwrap(), 3, "two, plus the one the lock spared");
    }

    #[test]
    fn an_over_aged_record_goes_even_when_the_list_is_short() {
        let mut t = store();
        let now = now_ms();
        let old =
            t.h.record(
                &at(now - 10 * 86_400_000),
                &frame(4, 4, [0; 4]),
                &settings(),
            )
            .unwrap();
        let fresh =
            t.h.record(&at(now), &frame(4, 4, [0; 4]), &settings())
                .unwrap();
        let freed =
            t.h.cleanup(&Retention {
                max_items: 100,
                max_age_ms: 86_400_000,
                ..unset()
            })
            .unwrap();
        assert_eq!(freed.ids, vec![old]);
        assert!(t.h.get(fresh).unwrap().is_some());
    }

    #[test]
    fn the_byte_budget_never_trades_a_newer_record_for_an_older_one() {
        let mut t = store();
        let big = frame(400, 400, [200, 30, 30, 255]);
        let a = t.h.record(&at(1_000), &big, &settings()).unwrap();
        let b = t.h.record(&at(2_000), &big, &settings()).unwrap();
        let c = t.h.record(&at(3_000), &big, &settings()).unwrap();
        let one = t.h.bytes_of(&t.h.get(c).unwrap().unwrap());
        let freed =
            t.h.cleanup(&Retention {
                max_bytes: one * 2 - 1,
                ..unset()
            })
            .unwrap();
        assert_eq!(freed.ids, vec![a, b], "the budget keeps the newest two");
        assert_eq!(t.h.count().unwrap(), 1);

        let used = t.h.usage().unwrap();
        assert_eq!(used.records, 1);
        assert_eq!(
            used.thumb_bytes,
            t.h.get(c).unwrap().unwrap().thumb.as_ref().unwrap().len() as u64
        );
        assert!(used.original_bytes > 0);
        assert!(used.db_bytes > 0);
        assert_eq!(
            used.total_bytes(),
            used.original_bytes + used.thumb_bytes + used.db_bytes
        );
    }

    #[test]
    fn a_zero_limit_disables_that_condition() {
        let mut t = store();
        for i in 0..3i64 {
            t.h.record(&at(i), &frame(4, 4, [0; 4]), &settings())
                .unwrap();
        }
        assert!(t.h.cleanup(&unset()).unwrap().ids.is_empty());
        assert_eq!(t.h.count().unwrap(), 3, "no limit is not \"delete all\"");
    }

    #[test]
    fn a_source_kind_is_spelled_the_same_in_rust_and_sql() {
        for s in [
            Source::Region,
            Source::Window,
            Source::Element,
            Source::Manual,
            Source::Repeat,
        ] {
            assert_eq!(Source::parse(s.as_str()), Some(s), "{}", s.as_str());
        }
        assert_eq!(Source::parse("everything"), None);
    }

    #[test]
    fn a_record_with_no_file_on_disk_falls_back_to_its_thumbnail() {
        let mut t = store();
        let id =
            t.h.record(&at(1), &frame(32, 32, [7, 7, 7, 255]), &settings())
                .unwrap();
        let rec = t.h.get(id).unwrap().unwrap();
        std::fs::remove_file(t.h.path_of(&rec).unwrap()).unwrap();
        // §8.7 is about the disk filling up; opening the list must not be.
        assert_eq!(t.h.frame(&rec).unwrap().width, 32);
        assert_eq!(t.h.usage().unwrap().original_bytes, 0);
    }

    #[test]
    fn a_failed_picture_write_leaves_no_row_behind() {
        let mut t = store();
        // Put a *file* where the dated folder has to go, so the write cannot
        // start. §5.14.1 must not gain a record that points at nothing.
        std::fs::create_dir(t.h.dir().join(PICTURES_DIR)).unwrap();
        std::fs::write(t.h.dir().join(PICTURES_DIR).join("1970-01-01"), b"occupied").unwrap();
        let err =
            t.h.record(&at(0), &frame(4, 4, [0; 4]), &settings())
                .expect_err("the day path is occupied by a file");
        assert!(matches!(err, HistoryError::Io(_)), "{err:?}");
        assert_eq!(
            t.h.count().unwrap(),
            0,
            "and the rollback took the row back"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        /// Whatever the limits, the cleaner leaves a history that satisfies them:
        /// the count is inside `max_items` up to what the lock spared, every
        /// record left is inside its retention time, and the bytes still there fit
        /// the budget. Nothing locked is ever cleaned up (PRD §5.14.4), and every
        /// survivor is still readable.
        #[test]
        fn cleanup_leaves_only_what_the_policy_allows(
            (n, items, days, budget, exempt) in arb_policy(),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let mut h = History::open(dir.path()).unwrap();
            let cfg = Settings { keep_originals: true, thumb_px: 32, ..Default::default() };
            let now = now_ms();
            let mut locked = Vec::new();
            for i in 0..n as i64 {
                // Six hours apart, so a one-day limit bites the oldest few and a
                // two-day limit the oldest half.
                let id = h
                    .record(
                        &at(now - (n as i64 - i) * 21_600_000),
                        &frame(8, 8, [i as u8, 0, 0, 255]),
                        &cfg,
                    )
                    .unwrap();
                if i % 3 == 0 {
                    h.set_locked(id, true).unwrap();
                    locked.push(id);
                }
            }
            let policy = Retention {
                max_items: items,
                max_age_ms: days as i64 * 86_400_000,
                max_bytes: budget as u64 * 512,
                exempt_locked: exempt,
            };
            let freed = h.cleanup(&policy).unwrap();
            let left = h.newest(4_096).unwrap();
            let spare: Vec<&Record> = if exempt {
                left.iter().filter(|r| r.locked).collect()
            } else {
                Vec::new()
            };

            if policy.max_items > 0 {
                prop_assert!(
                    left.len() <= policy.max_items + spare.len(),
                    "{} records for a limit of {}",
                    left.len(),
                    policy.max_items
                );
            }
            if policy.max_bytes > 0 {
                let bytes: u64 = left.iter().map(|r| h.bytes_of(r)).sum();
                let held: u64 = spare.iter().map(|r| h.bytes_of(r)).sum();
                prop_assert!(
                    bytes <= policy.max_bytes + held,
                    "{bytes} bytes left over a {} budget",
                    policy.max_bytes
                );
            }
            if policy.max_age_ms > 0 {
                for r in &left {
                    if !(exempt && r.locked) {
                        prop_assert!(
                            now_ms() - r.captured_at <= policy.max_age_ms,
                            "record {} stayed past its retention time",
                            r.id
                        );
                    }
                }
            }
            if exempt {
                for id in &locked {
                    prop_assert!(!freed.ids.contains(id), "a locked record was cleaned up");
                }
            }
            for r in &left {
                prop_assert_eq!(h.frame(r).unwrap().width, 8);
            }
        }
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];

    /// One monitor of a fake desktop: `x..x+w` wide, 100 tall, filled with `rgba`.
    fn monitor(id: &str, x: i32, w: u32, rgba: [u8; 4]) -> FrozenMonitor {
        FrozenMonitor {
            info: MonitorInfo {
                id: id.to_string(),
                name: id.to_string(),
                friendly: id.to_string(),
                bounds: PhysRect::new(x, 0, w, 100),
                scale: Scale::ONE,
                primary: x == 0,
                builtin: false,
            },
            frame: frame(w, 100, rgba),
        }
    }

    /// Two screens side by side, a 300x100 desktop: `a` red, `b` blue. The shape
    /// [`crate::capture`] uses for its stitching tests, so a stamp that lands
    /// correctly here lands correctly on the desktop the capture path already
    /// agrees about.
    fn desk_two() -> ScreenSnapshot {
        ScreenSnapshot {
            monitors: vec![monitor("a", 0, 200, RED), monitor("b", 200, 100, BLUE)],
            virtual_bounds: PhysRect::new(0, 0, 300, 100),
            taken_at_ms: 0,
            backend: "test",
        }
    }

    /// §5.14.2 第 4 步「标注」那一腿的形状：库里那张图落在**它自己记下的那块地方**，
    /// 而不是落在桌面正中，也不是落在屏幕之外。
    #[test]
    fn a_stamped_row_lands_on_its_own_rect() {
        let mut snap = desk_two();
        let rect = PhysRect::new(30, 40, 20, 10);
        let got = stamp_row(&mut snap, &rect, &frame(20, 10, GREEN)).unwrap();
        assert_eq!(got, 1, "这块框只碰了一块屏");
        let a = &snap.monitors[0].frame;
        assert_eq!(a.get(30, 40), GREEN, "框的左上角");
        assert_eq!(a.get(49, 49), GREEN, "框的右下角");
        assert_eq!(a.get(29, 40), RED, "框外一格不许被顺手动到");
        assert_eq!(a.get(50, 40), RED, "框外右边那一格同样");
        assert_eq!(
            snap.monitors[1].frame.get(0, 0),
            BLUE,
            "另一块屏上没有这一行的像素，就该一个字也不动"
        );
    }

    /// 跨屏缝的那一条：两块屏各得自己那半张图。这一条盯的是 `paste` 的偏移 ——
    /// 拿"重叠区左上角"当落点会把整张图钉到右屏的左边缘上，两块屏看到的都是左半张。
    #[test]
    fn a_row_across_the_seam_lands_on_both_screens() {
        let mut snap = desk_two();
        let rect = PhysRect::new(190, 10, 20, 10);
        let got = stamp_row(&mut snap, &rect, &frame(20, 10, GREEN)).unwrap();
        assert_eq!(got, 2);
        // 左屏拿到图的前 10 列，右屏拿到后 10 列 —— 中间那道缝两侧接得上同一张图。
        let a = &snap.monitors[0].frame;
        let b = &snap.monitors[1].frame;
        assert_eq!(a.get(195, 12), GREEN, "左屏那半张");
        assert_eq!(a.get(189, 12), RED, "左屏框外那一格");
        assert_eq!(b.get(0, 12), GREEN, "右屏那半张的第一列");
        assert_eq!(b.get(9, 12), GREEN, "右屏那半张的最后一列");
        assert_eq!(b.get(10, 12), BLUE, "右屏只该被涂 10 列，第 11 列还是桌面");
    }

    /// 那一行是在**已经不存在的屏**上框的：不编一块地方把它塞进去，也不假装画好了。
    /// [`HistoryError::OffSnapshot`] 与 [`HistoryError::EmptyRect`] 分两格答，正是为了让
    /// 界面能把"屏没了"和"这行本来就是坏的"说成两句话。
    #[test]
    fn a_row_that_cannot_land_is_refused_and_changes_nothing() {
        let mut snap = desk_two();
        let before = snap.clone();
        let far = PhysRect::new(4_000, 0, 20, 10);
        let e = stamp_row(&mut snap, &far, &frame(20, 10, GREEN)).unwrap_err();
        assert!(
            matches!(e, HistoryError::OffSnapshot(r) if r == far),
            "屏外的框该答屏外，答回来的是：{e}"
        );
        let empty = PhysRect::new(10, 10, 0, 20);
        let e = stamp_row(&mut snap, &empty, &frame(20, 10, GREEN)).unwrap_err();
        assert!(
            matches!(e, HistoryError::EmptyRect(r) if r == empty),
            "空的框该答空，答回来的是：{e}"
        );
        assert_eq!(snap.monitors[0].frame, before.monitors[0].frame);
        assert_eq!(snap.monitors[1].frame, before.monitors[1].frame);
    }

    /// `keep_originals` 关着时那一行手里只有缩略图。铺回快照时按**那一行的矩形**重画，
    /// 于是遮罩上看见的图与框住它的那只框是同一个尺寸 —— 不是把 10x4 的小图贴在
    /// 100x40 的框里、让用户去猜哪一部分是历史。
    #[test]
    fn a_small_picture_is_stamped_at_the_row_size() {
        let mut snap = desk_two();
        let rect = PhysRect::new(50, 20, 100, 40);
        let got = stamp_row(&mut snap, &rect, &frame(10, 4, GREEN)).unwrap();
        assert_eq!(got, 1);
        let a = &snap.monitors[0].frame;
        for (x, y) in [(50, 20), (99, 39), (149, 59)] {
            assert_eq!(a.get(x, y), GREEN, "({x},{y}) 该是这张图铺满的地方");
        }
        assert_eq!(a.get(49, 20), RED);
        assert_eq!(a.get(150, 20), RED);
    }

    /// 「保存这一枚不改动原记录」那一枚判据：一次回放（复制、存盘、贴图读的都是它）
    /// 之后，库里那一行**逐字段不变**、盘上那一帧一个字节不动、行数不增不减。
    /// 这条盯的是"读侧顺手写回"那一族：一次读如果把自己长成了新的一行，历史就从一台
    /// 录音机变成了会自己添话的人。
    #[test]
    fn replaying_a_row_leaves_the_record_exactly_as_it_was() {
        let mut t = store();
        let cfg = settings();
        assert!(cfg.keep_originals, "这条要盯的是原图那一帧，夹具得留着它");
        let id =
            t.h.record(&at(1_700_000_000_000), &frame(30, 40, GREEN), &cfg)
                .unwrap();
        let rec = t.h.get(id).unwrap().unwrap();
        let path = t.h.path_of(&rec).expect("原图就该在盘上");
        let bytes_before = std::fs::read(&path).unwrap();
        let count_before = t.h.count().unwrap();

        // 三步产品动作实际读的东西：那一行、那张图、那个路径。
        let again = t.h.get(rec.id).unwrap().unwrap();
        let pic = t.h.frame(&again).unwrap();
        assert_eq!(
            (pic.width, pic.height),
            (30, 40),
            "读回来的就是那一行的原尺寸"
        );
        // 再把它另存到别处去 —— 回放落盘那一腿做的正是这件事，只是换一个名字。
        let opts = encode::EncodeOptions {
            format: Format::Png,
            quality: 92,
            flatten_on_lossy: true,
        };
        let aside = t._dir.path().join("replayed.png");
        std::fs::write(&aside, encode::encode(&pic, &opts).unwrap()).unwrap();

        assert_eq!(again, rec, "读过一遍之后那一行不许有任何一处不同");
        assert_eq!(t.h.count().unwrap(), count_before, "读不该长出一行");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes_before,
            "盘上那一帧是历史本身，一次回放不能改写它"
        );
        assert!(aside.exists(), "另存的那一份才是这次动作留下的东西");
    }

    /// 一张 128 那一档的缩略图（`keep_originals` 关着的形状）走的是同一扇门：库里那一行
    /// 的图取回来是缩略图的尺寸，但**仍是那一行**，读它不改写它。
    #[test]
    fn reading_a_thumbnail_only_row_still_leaves_it_alone() {
        let mut t = store();
        let mut cfg = settings();
        cfg.keep_originals = false;
        let id =
            t.h.record(&at(1_700_000_000_000), &frame(64, 32, GREEN), &cfg)
                .unwrap();
        let rec = t.h.get(id).unwrap().unwrap();
        assert!(rec.path.is_none(), "不留原图就不该有路径");
        let pic = t.h.frame(&rec).unwrap();
        let mut snap = desk_two();
        // 那一行的矩形是 `at()` 给的 30x40，取回来的图是缩略图尺寸 —— 铺的时候按矩形重画。
        let rect = rec.rect;
        assert_eq!(stamp_row(&mut snap, &rect, &pic).unwrap(), 1);
        assert_eq!(
            t.h.get(id).unwrap().unwrap(),
            rec,
            "铺过一次之后库里那一行还是那一行"
        );
        let a = &snap.monitors[0].frame;
        assert_eq!((a.width, a.height), (200, 100), "动的是快照，不是别的什么");
        // 缩略图本身是 JPEG（[`HistoryStore::frame`] 就按 Jpg 解），所以这一格必然是
        // 「这张图的绿」而不是桌面那一片红/蓝 —— 而不是某个具体的字节：有损编码交回来的
        // 那一个通道差 1 是它本来的形状，把它判成坏等于要求 JPEG 交出无损的答。
        let got = a.get(rect.x as u32 + 15, rect.y as u32 + 20);
        assert!(
            got[1] >= 200 && got[0] < 60 && got[2] < 60 && got[3] == 255,
            "那一格该是这一行的图，交回来的是 {got:?}（桌面是 {RED:?}）"
        );
    }

    /// §8.7「空间不足」那一问：这一列此刻顶到某枚上限了没有。`0` 在 [`History::cleanup`] 与这里
    /// 是同一枚意思（这一枚上限没设），所以「没设限」不该问出一句「该清理了」——那是一句用户
    /// 没许过的话。天数那一枚问不出来：`Usage` 里没有时刻，替它猜就是第二个作者。
    #[test]
    fn the_cap_question_reads_the_same_zeros_the_cleaner_does() {
        let usage = |records: u64, bytes: u64| Usage {
            records,
            original_bytes: bytes,
            thumb_bytes: 0,
            db_bytes: 0,
        };
        let policy = Retention {
            max_items: 3,
            max_age_ms: 0,
            max_bytes: 1_000,
            exempt_locked: true,
        };
        assert!(!policy.at_cap(&usage(2, 500)), "两枚上限都还没到");
        assert!(policy.at_cap(&usage(3, 500)), "条数到了上限却没说");
        assert!(policy.at_cap(&usage(1, 1_000)), "字节到了上限却没说");
        assert!(!unset().at_cap(&usage(9_999, 9_999_999)), "一枚上限也没设却说顶到了");
        let items_only = Retention {
            max_bytes: 0,
            ..policy
        };
        assert!(
            !items_only.at_cap(&usage(1, 9_999_999)),
            "没设字节上限却按字节判到了"
        );
        let days_only = Retention {
            max_items: 0,
            max_bytes: 0,
            max_age_ms: 86_400_000,
            exempt_locked: true,
        };
        assert!(
            !days_only.at_cap(&usage(500, 500)),
            "只设了天数上限时这一问该答「没到」"
        );
    }

    fn arb_policy() -> impl Strategy<Value = (u8, usize, u8, u16, bool)> {
        (1u8..12, 0usize..6, 0u8..3, 0u16..12, any::<bool>())
    }
}
