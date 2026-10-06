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

use crate::capture::now_ms;
use crate::config::History as Settings;
use crate::encode::{self, Format};
use crate::frame::Frame;
use crate::geometry::PhysRect;
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

/// What one delete operation gave back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Freed {
    pub ids: Vec<i64>,
    pub bytes: u64,
}

pub const DB_FILE: &str = "history.db";
pub const PICTURES_DIR: &str = "history";
pub const SCHEMA_VERSION: i32 = 1;
/// §6.2 budgets ≈15 KB for a 256 px JPEG; this is the quality that lands there.
pub const THUMB_QUALITY: u8 = 80;

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
        self.collect(
            &format!("SELECT {COLUMNS} FROM captures ORDER BY captured_at DESC, id DESC LIMIT ?1"),
            params![limit as i64],
        )
    }

    /// What comes after `after` when the list scrolls down. The cursor is the
    /// record itself, not its id: two captures can share a `captured_at`, and the
    /// pair `(captured_at, id)` is what the ordering is defined on.
    pub fn older_than(&self, after: &Record, limit: usize) -> Result<Vec<Record>, HistoryError> {
        self.collect(
            &format!(
                "SELECT {COLUMNS} FROM captures WHERE (captured_at, id) < (?1, ?2) \
                 ORDER BY captured_at DESC, id DESC LIMIT ?3"
            ),
            params![after.captured_at, after.id, limit as i64],
        )
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

    pub fn set_note(&self, id: i64, note: &str) -> Result<(), HistoryError> {
        self.update("note", id, params![note, id])
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
    fn note_ocr_and_lock_all_round_trip() {
        let mut t = store();
        let id =
            t.h.record(&at(1), &frame(4, 4, [0, 0, 0, 255]), &settings())
                .unwrap();
        t.h.set_note(id, "发票").unwrap();
        t.h.set_ocr(id, Some("total 12.00")).unwrap();
        t.h.set_locked(id, true).unwrap();
        let rec = t.h.get(id).unwrap().unwrap();
        assert_eq!(rec.note, "发票");
        assert_eq!(rec.ocr_text.as_deref(), Some("total 12.00"));
        assert!(rec.locked);
        t.h.set_ocr(id, None).unwrap();
        assert_eq!(t.h.get(id).unwrap().unwrap().ocr_text, None);
        assert!(matches!(
            t.h.set_note(9999, "x"),
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

    fn arb_policy() -> impl Strategy<Value = (u8, usize, u8, u16, bool)> {
        (1u8..12, 0usize..6, 0u8..3, 0u16..12, any::<bool>())
    }
}
