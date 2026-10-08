//! The process's copy of the settings file (§6.1) - who reads it, who gets asked
//! for a value, and who writes it back.
//!
//! Before this module the file had no owner at all: `Config::load` and `Config::save`
//! existed and were tested against strings, and nothing in the product ever called
//! them. That left §9.2's "restart" promise unkept in the only place it can be kept -
//! the process that starts up after the one that remembered.
//!
//! The three touch points are the three that cost something:
//! * **start-up** reads the file (PRD §5.1.1: a damaged config never blocks startup);
//! * **opening the mask** hands the remembered pens to the layer about to draw
//!   (§5.7.1), which is on the hot path §9.2 caps at 400 ms;
//! * **ending a flow** writes what the layer learned back out.
//!
//! Saving happens at the end of a flow rather than at process exit on purpose: an
//! exit handler does not run when the user signs out or the process is killed, and
//! "the pen I just set" is exactly the thing that has to survive that.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Instant;

use falcon_core::config::{config_path, Config, LoadSource};

use crate::annotate::Layer;

/// Where the gauges put the file they are measured against.
///
/// This is a measurement hook, not a setting the product promises: reading the real
/// path would mean the first `--mask` run of the day touches
/// `%APPDATA%\ai-falconshot\config.toml`, and a gauge that writes at flow end would
/// then edit it. Nothing reads this unless it was set.
pub const ENV_OVERRIDE: &str = "FALCONSHOT_CONFIG";

/// The file to read, and the pin on where writes may go.
fn boot() -> (PathBuf, Option<PathBuf>) {
    let pinned = std::env::var_os(ENV_OVERRIDE)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let read = pinned
        .clone()
        .unwrap_or_else(|| config_path(&Config::default()));
    (read, pinned)
}

/// Everything the process knows about its settings, and what each touch point cost.
#[derive(Default)]
pub struct Settings {
    pub config: Config,
    /// Where this copy was read from.
    pub read_from: PathBuf,
    /// Where writes go. Not always `read_from`: the file itself can say where its
    /// data lives (§6.1's `advanced.data_dir`).
    pub write_to: PathBuf,
    pub source: LoadSource,
    pub warnings: Vec<String>,
    pub backup: Option<PathBuf>,
    /// Writes the machine refused. The values stay in memory either way, which is
    /// §9.4's rule for a failed save read for the settings file.
    pub problems: Vec<String>,
    pub load_ms: u64,
    pub applies: u32,
    pub apply_ms: u64,
    pub apply_max_ms: u64,
    pub saves: u32,
    pub save_ms: u64,
    pub save_max_ms: u64,
}

impl Settings {
    /// Read `path`, and write to `pinned` when the caller pinned one - the gauges do -
    /// or to wherever the file's own `advanced.data_dir` points otherwise.
    pub fn open(path: &Path, pinned: Option<&Path>) -> Settings {
        let started = Instant::now();
        let (config, report) = Config::load(path);
        let load_ms = started.elapsed().as_millis() as u64;
        let write_to = match pinned {
            Some(p) => p.to_path_buf(),
            None => config_path(&config),
        };
        Settings {
            config,
            read_from: path.to_path_buf(),
            write_to,
            source: report.source,
            warnings: report.warnings,
            backup: report.backup,
            load_ms,
            ..Settings::default()
        }
    }

    fn tag(&self) -> &'static str {
        match self.source {
            LoadSource::Defaults => "defaults",
            LoadSource::File => "file",
            LoadSource::Recovered => "recovered",
        }
    }

    /// The pens the file remembered, into the layer about to draw with them. Returns
    /// how many tools the file spoke about: `0` is a first run, and a restart test
    /// that reads back `0` proved nothing.
    ///
    /// `[annotation]` goes in first and `[annotation.tool_style]` after it, which is the
    /// order §5.7.1's fallback is written in: the scalars are the pen a tool starts with
    /// and a taught tool overrides them. Reversed, a per-tool entry would be overwritten
    /// by the default row on every start and the memory would not survive one restart.
    pub fn apply_to(&mut self, layer: &mut Layer) -> usize {
        let started = Instant::now();
        let pens = self.config.annotation.tool_style.clone();
        layer.apply_annotation_defaults(&self.config.annotation);
        layer.apply_tool_styles(&pens);
        let ms = started.elapsed().as_millis() as u64;
        self.applies += 1;
        self.apply_ms = ms;
        self.apply_max_ms = self.apply_max_ms.max(ms);
        pens.len()
    }

    /// What the layer learned, back into the config. `None` when nothing moved, and
    /// nothing is written then: a screenshot that never touched a pen must not
    /// rewrite the file.
    pub fn write_back(&mut self, layer: &Layer) -> Option<u64> {
        let learned = layer.tool_styles();
        if learned == self.config.annotation.tool_style {
            return None;
        }
        self.config.annotation.tool_style = learned;
        Some(self.save())
    }

    /// One atomic write, timed. A refusal is recorded and the values are kept.
    pub fn save(&mut self) -> u64 {
        let started = Instant::now();
        match self.config.save(&self.write_to) {
            Ok(()) => self.saves += 1,
            Err(e) => self
                .problems
                .push(format!("{}: {e}", self.write_to.display())),
        }
        let ms = started.elapsed().as_millis() as u64;
        self.save_ms = ms;
        self.save_max_ms = self.save_max_ms.max(ms);
        ms
    }

    /// One line, because every other gauge line is one line.
    pub fn line(&self) -> String {
        format!(
            "read={} load={}ms applies={} apply={}/{}ms saves={} save={}/{}ms from={} write_to={}",
            self.tag(),
            self.load_ms,
            self.applies,
            self.apply_ms,
            self.apply_max_ms,
            self.saves,
            self.save_ms,
            self.save_max_ms,
            self.read_from.display(),
            self.write_to.display(),
        )
    }

    /// What had to be corrected and what the machine refused - empty on a clean first
    /// run, which is why the caller prints it only when there is something to say.
    pub fn notes(&self) -> Vec<String> {
        let mut out = self.warnings.clone();
        out.extend(self.problems.iter().cloned());
        if let Some(b) = &self.backup {
            out.push(format!("unreadable config moved aside to {}", b.display()));
        }
        out
    }
}

static SETTINGS: OnceLock<Mutex<Settings>> = OnceLock::new();

/// The one copy the process runs on. Asking for it is what loads the file, so no
/// caller can get an empty `Settings` by asking before `start()` ran - the
/// alternative was a load in the middle of opening the mask, which is the hot path.
fn cell() -> &'static Mutex<Settings> {
    SETTINGS.get_or_init(|| {
        let (read, pinned) = boot();
        Mutex::new(Settings::open(&read, pinned.as_deref()))
    })
}

pub fn with<R>(f: impl FnOnce(&mut Settings) -> R) -> R {
    let mut guard = cell().lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut guard)
}

/// Take the file at start-up and return the line the gauges print.
pub fn start() -> String {
    with(|s| s.line())
}

/// §5.7.1's memory, handed over as the mask opens.
pub fn apply_to(layer: &mut Layer) -> usize {
    with(|s| s.apply_to(layer))
}

/// The flow is over: keep what the layer learned, and write if that moved anything.
pub fn write_back(layer: &Layer) -> Option<u64> {
    with(|s| s.write_back(layer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use falcon_core::annotation::model::Kind;
    use falcon_core::config::FILE_NAME;

    /// A directory under the OS temp dir, one per test. `%APPDATA%` is not a test
    /// fixture, and none of these reach it: every `Settings` here is opened against
    /// an explicit path rather than through the process global.
    fn sandbox(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("falconshot-settings-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("sandbox");
        dir
    }

    fn file_in(dir: &Path) -> PathBuf {
        dir.join(FILE_NAME)
    }

    /// The only way a test here may open settings that write.
    ///
    /// `None` for the pin is not "stay in this directory" - it is "ask the file where
    /// its data lives", and an empty `advanced.data_dir` answers with the real
    /// `%APPDATA%\ai-falconshot\config.toml`. The first draft of these tests made that
    /// mistake three times, and the writes landed there: a sandbox directory in the
    /// assertion text is not a sandbox.
    fn pinned(dir: &Path) -> Settings {
        assert!(
            dir.starts_with(std::env::temp_dir()),
            "settings tests may only write under the temp dir, got {dir:?}"
        );
        let path = file_in(dir);
        Settings::open(&path, Some(&path))
    }

    fn code_of(kind: Kind) -> i32 {
        crate::annotate::TOOLS
            .iter()
            .position(|t| *t == Some(kind))
            .expect("tool has a button") as i32
    }

    /// Teach one tool a pen the way the toolbar does: pick the button, then set the
    /// knobs. A pen set with no tool selected is forgotten by the layer (§5.7.1).
    fn teach(layer: &mut Layer, kind: Kind, rgba: [u8; 4], width: u32) {
        layer.select_tool(code_of(kind));
        layer.set_color(rgba);
        layer.set_width(width);
    }

    #[test]
    fn a_first_run_reads_defaults_and_leaves_no_file_behind() {
        let dir = sandbox("first-run");
        let path = file_in(&dir);
        let s = pinned(&dir);
        assert!(matches!(s.source, LoadSource::Defaults));
        assert_eq!(s.config, Config::default());
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        // Reading the settings must not create them: the file appears when there is
        // something of the user's to keep, not when the product first wakes up.
        assert!(!path.exists(), "startup wrote a config file");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_pen_taught_in_one_process_is_drawn_with_in_the_next() {
        // §9.2's sentence end to end, through a file rather than through a test's
        // memory - the half P17 left as two seams.
        let dir = sandbox("restart");
        let path = file_in(&dir);

        let mut s = pinned(&dir);
        let mut layer = Layer::default();
        assert_eq!(
            s.apply_to(&mut layer),
            0,
            "first run has nothing to remember"
        );
        teach(&mut layer, Kind::Rect, [1, 2, 3, 240], 7);
        let ms = s
            .write_back(&layer)
            .expect("a pen moved, so a write happens");
        assert!(path.exists(), "the write did not land");
        assert_eq!(s.saves, 1);
        assert!(ms < 5_000, "a save that costs {ms} ms is not a save");

        // The next process: the same path, nothing in memory.
        let mut s2 = pinned(&dir);
        assert!(matches!(s2.source, LoadSource::File));
        assert_eq!(s2.config.annotation.tool_style.len(), 1);
        let mut after = Layer::default();
        assert_eq!(s2.apply_to(&mut after), 1, "the file handed over one pen");
        // And the toolbar still has to see it: selecting the tool is what shows the
        // pen, so this is the readback a user gets rather than a field of the table.
        after.select_tool(code_of(Kind::Rect));
        assert_eq!(after.color(), [1, 2, 3, 240], "the pen did not come back");
        assert_eq!(after.width(), 7);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_row_written_by_hand_reaches_the_pen_across_a_real_file() {
        // The `[annotation]` scalars, through bytes on a disk rather than through a
        // `Config` built in a test - which is the only form of this row a user can
        // actually produce. Until this round the file accepted all four keys, kept them
        // through a save, and handed `apply_to` a table that read none of them, so every
        // test in the tree that touched a pen went through `tool_style` instead and the
        // row stayed unmeasured.
        let dir = sandbox("annotation-row");
        let path = file_in(&dir);
        std::fs::write(
            &path,
            "[annotation]\nstroke_color = \"#00FF00FF\"\nstroke_width = 5\n\
             font_size = 32\nfont_family = \"Consolas\"\n",
        )
        .expect("write the hand-made file");

        let mut s = pinned(&dir);
        assert!(matches!(s.source, LoadSource::File));
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        let mut layer = Layer::default();
        assert_eq!(
            s.apply_to(&mut layer),
            0,
            "the row is not a tool_style table"
        );
        assert_eq!(layer.color(), [0, 255, 0, 255], "the file's colour lost");
        assert_eq!(layer.width(), 5);
        assert_eq!(layer.font_px(), 32);
        assert_eq!(layer.font_family(), "Consolas");
        assert!(layer.problems.is_empty(), "{:?}", layer.problems);
        // And the row that says nothing about a tool still must not invent one: a save
        // here would rewrite the user's file with a pen they never set.
        assert_eq!(layer.tool_styles().len(), 0);
        assert_eq!(s.write_back(&layer), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_flow_that_never_touched_a_pen_writes_nothing() {
        let dir = sandbox("no-move");
        let path = file_in(&dir);
        let mut s = pinned(&dir);
        assert_eq!(s.write_back(&Layer::default()), None);
        assert_eq!(s.saves, 0);
        assert!(!path.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_second_flow_learns_what_the_first_one_wrote() {
        // Two rounds against the *same* Settings, because the product keeps one
        // process up: the file must not grow a tool per screenshot, and a pen that
        // is already stored is not a change.
        let dir = sandbox("two-flows");
        let mut s = pinned(&dir);

        let mut first = Layer::default();
        teach(&mut first, Kind::Marker, [9, 8, 7, 255], 40);
        s.write_back(&first);

        let mut second = Layer::default();
        assert_eq!(s.apply_to(&mut second), 1);
        teach(&mut second, Kind::Marker, [9, 8, 7, 255], 40);
        assert_eq!(
            s.write_back(&second),
            None,
            "the same pen set twice is not a change"
        );
        assert_eq!(s.saves, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_damaged_file_is_kept_and_the_app_still_opens() {
        let dir = sandbox("damaged");
        let path = file_in(&dir);
        std::fs::write(&path, b"[[[ not toml at all\n\t\n[").expect("write garbage");

        let mut s = Settings::open(&path, Some(&path));
        assert!(matches!(s.source, LoadSource::Recovered));
        assert_eq!(s.config, Config::default(), "defaults must win");
        let backup = s.backup.clone().expect("the bad file was kept");
        assert!(backup.exists(), "{backup:?}");
        assert!(!s.warnings.is_empty());
        assert_eq!(s.notes().len(), 2, "{:?}", s.notes());

        // "Recovered" is not "read-only": the next pen still has somewhere to go.
        let mut layer = Layer::default();
        teach(&mut layer, Kind::Rect, [4, 5, 6, 255], 3);
        s.write_back(&layer);
        let again = Settings::open(&path, Some(&path));
        assert!(matches!(again.source, LoadSource::File));
        assert_eq!(again.config.annotation.tool_style.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_file_decides_where_the_next_write_goes() {
        // `advanced.data_dir` (§6.1) is a redirect set *inside* the file, so reading
        // one place and writing another is the configured behaviour, not a bug.
        let read_dir = sandbox("redirect-read");
        let data_dir = sandbox("redirect-data");
        let path = file_in(&read_dir);
        let mut cfg = Config::default();
        cfg.advanced.data_dir = data_dir.display().to_string();
        std::fs::write(&path, cfg.export_text().unwrap()).expect("seed");

        let s = Settings::open(&path, None);
        assert_eq!(s.read_from, path);
        assert_eq!(s.write_to, file_in(&data_dir));

        // A gauge that pinned its own file keeps its writes there instead.
        let pinned = Settings::open(&path, Some(&path));
        assert_eq!(pinned.write_to, path);
        std::fs::remove_dir_all(&read_dir).ok();
        std::fs::remove_dir_all(&data_dir).ok();
    }

    #[test]
    fn a_write_refused_keeps_the_values_and_says_so() {
        let dir = sandbox("refused");
        // A directory sitting where the file should be: the write cannot land.
        let blocked = file_in(&dir);
        std::fs::create_dir_all(&blocked).expect("block the path with a directory");
        let mut s = Settings::open(&blocked, Some(&blocked));
        let mut layer = Layer::default();
        teach(&mut layer, Kind::Rect, [7, 7, 7, 255], 5);
        s.write_back(&layer);
        assert_eq!(s.saves, 0);
        assert_eq!(s.problems.len(), 1, "{:?}", s.problems);
        assert_eq!(s.config.annotation.tool_style.len(), 1, "values kept");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_line_names_every_cost_the_plan_asks_for() {
        // The report is the measurement: a number that is not printed is a number
        // nobody can check against the 400 ms line.
        let dir = sandbox("line");
        let path = file_in(&dir);
        let s = Settings::open(&path, Some(&path));
        let line = s.line();
        for part in [
            "read=defaults",
            "load=",
            "applies=0",
            "saves=0",
            "write_to=",
        ] {
            assert!(line.contains(part), "{line} lacks {part}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
