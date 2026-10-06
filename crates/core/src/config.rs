//! Configuration (PRD §5.20): the eleven settings categories, an atomic save
//! that keeps the user's comments, and recovery from a damaged file (§5.1.1).
//!
//! Two representations coexist on purpose:
//! * the typed structs below, which every product module reads;
//! * a dotted-path view (`get_str`/`set_str`) over the same data, which is what
//!   the QML settings page binds to — one code path for a hundred widgets
//!   instead of a property per setting.

use crate::encode::Format;
use crate::naming::Collision;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(String),
    #[error("bad toml: {0}")]
    Parse(String),
    #[error("no such setting: {0}")]
    UnknownKey(String),
    #[error("{key}: {reason}")]
    InvalidValue { key: String, reason: String },
}

/// Keys that need the process to restart before they take effect (§5.20.2:
/// "需要重启的设置应明确标记").
pub const RESTART_KEYS: &[&str] = &[
    "general.language",
    "general.launch_at_startup",
    "advanced.data_dir",
    "advanced.cli_enabled",
];

/// §5.16.1 bindable action ids. Hot corners and per-app exclusions name these.
pub const ACTIONS: &[&str] = &[
    "capture",
    "capture_custom",
    "capture_repeat",
    "capture_active_window",
    "paste_pin",
    "toggle_pins",
    "switch_group",
    "solo",
    "pin_edit",
    "custom_task",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    /// Full-screen mask with free selection (§5.3.1).
    #[default]
    Region,
    Window,
    /// Element under the cursor (§5.3.3).
    Element,
    Fullscreen,
    /// Re-shoot the previous rect (§5.2.3).
    Repeat,
}

/// §5.9.15: how a pin treats transparent pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaBg {
    /// Not drawn, not clickable.
    #[default]
    Transparent,
    /// Looks transparent, whole window clickable.
    Pseudo,
    CheckerDark,
    CheckerLight,
}

/// §5.2.2: the tool the annotation stage opens with. The config has to reject a
/// typo without knowing how to draw, so the id list lives here.
pub const ANNOTATION_TOOLS: &[&str] = &[
    "rect",
    "ellipse",
    "line",
    "arrow",
    "curve",
    "polyline",
    "polygon",
    "text",
    "number",
    "highlighter",
    "blur",
    "mosaic",
    "pen",
    "eraser",
    "counter",
    "stamp",
    "callout",
    "crop",
    "spotlight",
    "measure",
    "select",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// §5.1.2 开机自启.
    pub launch_at_startup: bool,
    pub start_minimized: bool,
    pub show_tray: bool,
    /// §5.1.3 closing the window keeps the app running.
    pub close_to_tray: bool,
    pub confirm_exit: bool,
    /// `auto`, or a BCP 47 tag such as `zh-CN` / `en`.
    pub language: String,
}

impl Default for General {
    fn default() -> Self {
        Self {
            // PRD §5.1.2 makes autostart a user choice, never a default.
            launch_at_startup: false,
            start_minimized: true,
            show_tray: true,
            close_to_tray: true,
            confirm_exit: false,
            language: "auto".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Capture {
    pub default_mode: CaptureMode,
    /// §5.5.3 cursor inclusion.
    pub include_cursor: bool,
    /// §5.2.5 freeze the desktop before the mask appears. Off saves the freeze
    /// cost but shows a live screen while selecting.
    pub freeze_frame: bool,
    /// §5.3.3 highlight the window/element under the cursor.
    pub highlight_element: bool,
    /// §5.4.1 magnifier geometry.
    pub magnifier_radius: u32,
    pub magnifier_zoom: u32,
    pub magnifier_grid: bool,
    pub magnifier_coordinates: bool,
    /// A [`crate::colors::ColorFormat`] name.
    pub magnifier_color_format: String,
    /// §5.3.5 the live size readout.
    pub show_dimensions: bool,
    /// §5.2.4 default for the delayed-capture picker; 0 shoots immediately.
    pub delay_seconds: u32,
    /// §5.3.6 snap selection edges to the window under the cursor.
    pub snap_to_window: bool,
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            default_mode: CaptureMode::Region,
            include_cursor: false,
            freeze_frame: true,
            highlight_element: true,
            magnifier_radius: 8,
            magnifier_zoom: 8,
            magnifier_grid: true,
            magnifier_coordinates: true,
            magnifier_color_format: "hex_upper".into(),
            show_dimensions: true,
            delay_seconds: 0,
            snap_to_window: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Annotation {
    /// Tool id activated on entry (§5.7); empty = keep the last used tool.
    pub default_tool: String,
    pub stroke_width: u32,
    pub stroke_color: String,
    pub fill_color: String,
    pub font_family: String,
    pub font_size: u32,
    /// `straight` | `elbow` | `curve` (§5.7.4).
    pub arrow_style: String,
    /// §5.7.7 mosaic / §5.7.8 blur strength in pixels.
    pub blur_radius: u32,
    /// §5.20.3 the palette travels with exported settings.
    pub palette: Vec<String>,
    /// §7.2 undo depth; plan §6.4 fixed 200 steps and a 400 ms merge window.
    pub undo_limit: usize,
    pub undo_merge_ms: u32,
    /// §5.7.1 keep the toolbar visible while hovering the selection.
    pub show_toolbar_on_hover: bool,
}

impl Default for Annotation {
    fn default() -> Self {
        Self {
            default_tool: String::new(),
            stroke_width: 3,
            stroke_color: "#E81123".into(),
            fill_color: "#00000000".into(),
            font_family: "Segoe UI".into(),
            font_size: 18,
            arrow_style: "straight".into(),
            blur_radius: 12,
            palette: default_palette(),
            undo_limit: 200,
            undo_merge_ms: 400,
            show_toolbar_on_hover: true,
        }
    }
}

fn default_palette() -> Vec<String> {
    [
        "#E81123", "#F38B00", "#FFB900", "#7A7A7A", "#FFFFFF", "#000000", "#0078D7", "#16C60C",
        "#B4009E", "#00B7C3",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pin {
    /// §5.9.1 default scale percent.
    pub default_zoom: u32,
    /// §5.9.2 percent.
    pub default_opacity: u32,
    pub default_topmost: bool,
    pub default_click_through: bool,
    pub alpha_bg: AlphaBg,
    /// §5.9.6 the mouse wheel changes size.
    pub wheel_resizes: bool,
    /// §5.10.4 clicking a stacked pin cycles instead of raising.
    pub cycle_on_click: bool,
    /// §6.3 crash-recovery snapshot cadence in seconds; 0 disables.
    pub state_save_seconds: u32,
    /// §5.11.1 Solo dims the others instead of hiding them.
    pub solo_dim_opacity: u32,
}

impl Default for Pin {
    fn default() -> Self {
        Self {
            default_zoom: 100,
            default_opacity: 100,
            default_topmost: true,
            default_click_through: false,
            alpha_bg: AlphaBg::Transparent,
            wheel_resizes: true,
            cycle_on_click: false,
            state_save_seconds: 5,
            solo_dim_opacity: 35,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Output {
    /// §5.5.1 always put the result on the clipboard.
    pub auto_copy: bool,
    /// §5.5.2 automatic file save without a dialog.
    pub auto_save: bool,
    /// Empty = the pictures folder (§6.2).
    pub save_dir: String,
    pub format: Format,
    /// §5.5.5 JPG quality.
    pub jpeg_quality: u32,
    /// §5.5.3 naming template, see [`crate::naming`].
    pub naming_template: String,
    pub collision: Collision,
    /// §5.5.6 rounded corners; 0 off.
    pub corner_radius: u32,
    /// §5.5.7 drop shadow; 0 off.
    pub shadow_size: u32,
    /// §5.5.4 open the destination after a manual save.
    pub reveal_after_save: bool,
    /// §5.5.11 share targets shown in the toolbar.
    pub share_targets: Vec<String>,
    /// §5.5.12 "send to app" entries: `name|executable`.
    pub send_to_apps: Vec<String>,
    /// §5.5.10 refresh the last output instead of making a new one.
    pub reuse_last_file: bool,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            auto_copy: true,
            auto_save: false,
            save_dir: String::new(),
            format: Format::Png,
            jpeg_quality: 92,
            naming_template: crate::naming::DEFAULT_TEMPLATE.into(),
            collision: Collision::Increment,
            corner_radius: 0,
            shadow_size: 0,
            reveal_after_save: false,
            share_targets: Vec::new(),
            send_to_apps: Vec::new(),
            reuse_last_file: false,
        }
    }
}

/// §5.16.2 one exclusion rule.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppExclusion {
    /// Executable name or path pattern, matched case-insensitively.
    pub app: String,
    /// Action ids this rule silences; empty = every global hotkey.
    pub actions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkey {
    /// Action id → key spec (`ctrl+shift+x`).
    pub actions: BTreeMap<String, String>,
    pub exclusions: Vec<AppExclusion>,
}

impl Default for Hotkey {
    fn default() -> Self {
        let mut actions = BTreeMap::new();
        actions.insert("capture".into(), "ctrl+shift+a".into());
        actions.insert("capture_custom".into(), "ctrl+shift+f".into());
        actions.insert("capture_repeat".into(), "ctrl+shift+x".into());
        actions.insert("capture_active_window".into(), "print".into());
        actions.insert("paste_pin".into(), "ctrl+shift+v".into());
        actions.insert("toggle_pins".into(), "ctrl+shift+p".into());
        actions.insert("switch_group".into(), "ctrl+shift+g".into());
        actions.insert("solo".into(), "ctrl+shift+s".into());
        actions.insert("pin_edit".into(), "ctrl+shift+e".into());
        Hotkey {
            actions,
            exclusions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotCorner {
    pub enabled: bool,
    /// Corner id → action id; an empty value means "this corner is off".
    pub corners: BTreeMap<String, String>,
    /// Distance from the vertex that still counts as "the corner", in DIP.
    pub trigger_dip: u32,
    /// Require a modifier so maximising a window never fires one by accident.
    pub require_ctrl: bool,
    pub require_shift: bool,
    /// Ignore repeat triggers inside this window (§5.15.3).
    pub repeat_guard_ms: u32,
}

impl Default for HotCorner {
    fn default() -> Self {
        let mut corners = BTreeMap::new();
        corners.insert("top_left".into(), String::new());
        corners.insert("top_right".into(), String::new());
        corners.insert("bottom_left".into(), String::new());
        corners.insert("bottom_right".into(), "capture".into());
        HotCorner {
            enabled: false,
            corners,
            trigger_dip: 8,
            require_ctrl: false,
            require_shift: false,
            repeat_guard_ms: 800,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
    /// §5.19.2 主题色.
    pub accent: String,
    /// §5.19.4 toolbar side: `auto` | `top` | `bottom` | `left` | `right`.
    pub toolbar_placement: String,
    /// §5.19.4 interface font size, in points.
    pub ui_font_size: u32,
    /// §5.19.3 magnifier frame.
    pub magnifier_border: u32,
    pub magnifier_corner_radius: u32,
    pub magnifier_foreground: String,
    pub magnifier_background: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            accent: "#0078D7".into(),
            toolbar_placement: "auto".into(),
            ui_font_size: 9,
            magnifier_border: 1,
            magnifier_corner_radius: 6,
            magnifier_foreground: "#FFFFFF".into(),
            magnifier_background: "#1F1F1F".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub enabled: bool,
    /// §5.14.4 the three retention conditions; 0 disables that one.
    pub max_items: u32,
    pub max_age_days: u32,
    pub max_size_mb: u32,
    /// §5.14.2 keep the full-resolution original on disk.
    pub keep_originals: bool,
    /// §6.2 thumbnail edge in pixels.
    pub thumb_px: u32,
    /// §5.14.5 locked entries survive cleanup.
    pub exempt_locked: bool,
}

impl Default for History {
    fn default() -> Self {
        Self {
            enabled: true,
            max_items: 1000,
            max_age_days: 90,
            max_size_mb: 4096,
            keep_originals: true,
            thumb_px: 256,
            exempt_locked: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Advanced {
    /// §5.17 register the `snip` command line entry point.
    pub cli_enabled: bool,
    /// `off` | `error` | `warn` | `info` | `debug` | `trace`.
    pub log_level: String,
    /// Empty = the platform default under `%APPDATA%` (§6.1).
    pub data_dir: String,
    /// §5.6 run OCR / QR recognition right after a capture.
    pub recognize_text: bool,
    /// §7.2 write a recovery snapshot of live pins.
    pub crash_recovery: bool,
    /// §7.5 keep the frozen frame in memory only, never on disk.
    pub privacy_no_temp_files: bool,
}

impl Default for Advanced {
    fn default() -> Self {
        Self {
            cli_enabled: false,
            log_level: "info".into(),
            data_dir: String::new(),
            recognize_text: false,
            crash_recovery: true,
            privacy_no_temp_files: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct About {
    /// §5.21.1
    pub auto_check_updates: bool,
    pub check_every_days: u32,
    /// `stable` | `beta` | `nightly`.
    pub channel: String,
    /// §5.21.3 attach diagnostics to feedback.
    pub send_diagnostics: bool,
}

impl Default for About {
    fn default() -> Self {
        Self {
            auto_check_updates: true,
            check_every_days: 7,
            channel: "stable".into(),
            send_diagnostics: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub capture: Capture,
    pub annotation: Annotation,
    pub pin: Pin,
    pub output: Output,
    pub hotkey: Hotkey,
    pub hot_corner: HotCorner,
    pub appearance: Appearance,
    pub history: History,
    pub advanced: Advanced,
    pub about: About,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoadSource {
    /// No file yet; the app runs on defaults.
    #[default]
    Defaults,
    /// Read straight from the file.
    File,
    /// The file could not be understood; it was moved aside (§5.1.1).
    Recovered,
}

#[derive(Clone, Debug, Default)]
pub struct LoadReport {
    pub source: LoadSource,
    /// Where the unreadable file went.
    pub backup: Option<PathBuf>,
    /// Values that were out of range and had to be corrected.
    pub warnings: Vec<String>,
}

pub const FILE_NAME: &str = "config.toml";

impl Config {
    /// An unreadable config never blocks startup (§5.1.1): defaults win, and the
    /// damaged file is kept as `config.toml.broken-<ts>`.
    pub fn load(path: &Path) -> (Config, LoadReport) {
        let mut report = LoadReport::default();
        let mut cfg = match std::fs::read_to_string(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.source = LoadSource::Defaults;
                Config::default()
            }
            Err(e) => {
                report.source = LoadSource::Defaults;
                report.warnings.push(format!("{}: {e}", path.display()));
                Config::default()
            }
            Ok(text) => match toml_edit::de::from_str::<Config>(&text) {
                Ok(cfg) => {
                    report.source = LoadSource::File;
                    cfg
                }
                Err(e) => {
                    report.source = LoadSource::Recovered;
                    report.warnings.push(format!("{}: {e}", path.display()));
                    report.backup = quarantine(path);
                    Config::default()
                }
            },
        };
        report.warnings.extend(cfg.validate());
        (cfg, report)
    }

    /// Atomic write that keeps the existing file's comments and layout (§6.1).
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| ConfigError::Io(format!("{}: {e}", dir.display())))?;
            }
        }
        let fresh = self.to_document()?;
        let text = match std::fs::read_to_string(path) {
            Ok(existing) => match existing.parse::<DocumentMut>() {
                Ok(mut doc) => {
                    merge_table(doc.as_table_mut(), fresh.as_table());
                    doc.to_string()
                }
                // Already unreadable, so a clean rewrite loses nothing usable.
                Err(_) => fresh.to_string(),
            },
            Err(_) => fresh.to_string(),
        };
        atomic_write(path, text.as_bytes())
    }

    pub fn to_document(&self) -> Result<DocumentMut, ConfigError> {
        let mut doc =
            toml_edit::ser::to_document(self).map_err(|e| ConfigError::Parse(e.to_string()))?;
        promote_tables(doc.as_table_mut());
        Ok(doc)
    }

    pub fn export_text(&self) -> Result<String, ConfigError> {
        Ok(self.to_document()?.to_string())
    }

    /// §5.20.3 step 4: a file is parsed and corrected before anything is touched.
    pub fn parse_import(text: &str) -> Result<Config, ConfigError> {
        let mut cfg: Config =
            toml_edit::de::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        cfg.validate();
        Ok(cfg)
    }

    /// Every reachable setting as `section.key` → printed value, sorted. The
    /// settings page enumerates this, so a key missing here is a key no widget
    /// can bind to either.
    pub fn flat(&self) -> Result<BTreeMap<String, String>, ConfigError> {
        let doc = self.to_document()?;
        let mut out = BTreeMap::new();
        flatten(doc.as_table(), "", &mut out);
        Ok(out)
    }

    pub fn get_str(&self, key: &str) -> Option<String> {
        self.flat().ok()?.get(key).cloned()
    }

    pub fn has_key(&self, key: &str) -> bool {
        self.flat().map(|f| f.contains_key(key)).unwrap_or(false)
    }

    /// Write a dotted setting from the string a settings widget produced. The
    /// stored type decides how the text is parsed, so a bool never becomes the
    /// string `"true"`.
    pub fn set_str(&mut self, key: &str, raw: &str) -> Result<(), ConfigError> {
        let parts: Vec<&str> = key.split('.').collect();
        if parts.len() < 2 || parts.iter().any(|p| p.is_empty()) {
            return Err(ConfigError::UnknownKey(key.to_string()));
        }
        let mut doc = self.to_document()?;
        let old = find_value(&doc, &parts)
            .cloned()
            .ok_or_else(|| ConfigError::UnknownKey(key.to_string()))?;
        let next = coerce(&old, parts[parts.len() - 1], raw)?;
        let slot = find_value_mut(&mut doc, &parts)
            .ok_or_else(|| ConfigError::UnknownKey(key.to_string()))?;
        let decor = slot.decor().clone();
        *slot = next;
        slot.decor_mut().clone_from(&decor);
        *self = toml_edit::de::from_document::<Config>(doc)
            .map_err(|e| ConfigError::Parse(e.to_string()))?;
        Ok(())
    }

    /// §5.20.3 step 4: what the incoming file would overwrite, item by item.
    pub fn preview_import(&self, incoming: &Config, scope: ImportScope) -> Vec<ImportChange> {
        let before = self.flat().unwrap_or_default();
        let after = incoming.flat().unwrap_or_default();
        let mut out = Vec::new();
        for (key, to) in &after {
            if !scope.covers(key) {
                continue;
            }
            let Some(from) = before.get(key) else {
                continue;
            };
            if from != to {
                out.push(ImportChange {
                    key: key.clone(),
                    from: from.clone(),
                    to: to.clone(),
                    restart_required: RESTART_KEYS.contains(&key.as_str()),
                });
            }
        }
        out
    }

    /// §5.20.3 step 5.
    pub fn apply_import(
        &mut self,
        incoming: &Config,
        scope: ImportScope,
    ) -> Result<(), ConfigError> {
        for change in self.preview_import(incoming, scope) {
            self.set_str(&change.key, &change.to)?;
        }
        Ok(())
    }

    /// Clamp and correct unusable values, reporting each change so the settings
    /// page can say so instead of silently disagreeing (§8.3).
    pub fn validate(&mut self) -> Vec<String> {
        let mut w = Vec::new();
        clamp(
            &mut w,
            "capture.magnifier_radius",
            &mut self.capture.magnifier_radius,
            1,
            64,
        );
        clamp(
            &mut w,
            "capture.magnifier_zoom",
            &mut self.capture.magnifier_zoom,
            1,
            64,
        );
        clamp(
            &mut w,
            "capture.delay_seconds",
            &mut self.capture.delay_seconds,
            0,
            300,
        );
        clamp(
            &mut w,
            "pin.default_zoom",
            &mut self.pin.default_zoom,
            10,
            800,
        );
        clamp(
            &mut w,
            "pin.default_opacity",
            &mut self.pin.default_opacity,
            10,
            100,
        );
        clamp(
            &mut w,
            "pin.solo_dim_opacity",
            &mut self.pin.solo_dim_opacity,
            10,
            100,
        );
        clamp(
            &mut w,
            "pin.state_save_seconds",
            &mut self.pin.state_save_seconds,
            0,
            3600,
        );
        clamp(
            &mut w,
            "appearance.ui_font_size",
            &mut self.appearance.ui_font_size,
            6,
            48,
        );
        clamp(
            &mut w,
            "appearance.magnifier_border",
            &mut self.appearance.magnifier_border,
            0,
            8,
        );
        clamp(
            &mut w,
            "appearance.magnifier_corner_radius",
            &mut self.appearance.magnifier_corner_radius,
            0,
            48,
        );
        clamp(
            &mut w,
            "hot_corner.trigger_dip",
            &mut self.hot_corner.trigger_dip,
            1,
            100,
        );
        clamp(
            &mut w,
            "hot_corner.repeat_guard_ms",
            &mut self.hot_corner.repeat_guard_ms,
            100,
            10_000,
        );
        clamp(
            &mut w,
            "output.jpeg_quality",
            &mut self.output.jpeg_quality,
            1,
            100,
        );
        clamp(
            &mut w,
            "output.corner_radius",
            &mut self.output.corner_radius,
            0,
            64,
        );
        clamp(
            &mut w,
            "output.shadow_size",
            &mut self.output.shadow_size,
            0,
            96,
        );
        clamp(
            &mut w,
            "annotation.stroke_width",
            &mut self.annotation.stroke_width,
            1,
            64,
        );
        clamp(
            &mut w,
            "annotation.font_size",
            &mut self.annotation.font_size,
            6,
            200,
        );
        clamp(
            &mut w,
            "annotation.blur_radius",
            &mut self.annotation.blur_radius,
            1,
            96,
        );
        clamp(
            &mut w,
            "annotation.undo_merge_ms",
            &mut self.annotation.undo_merge_ms,
            0,
            5000,
        );
        clamp(
            &mut w,
            "history.max_items",
            &mut self.history.max_items,
            0,
            100_000,
        );
        clamp(
            &mut w,
            "history.max_age_days",
            &mut self.history.max_age_days,
            0,
            3650,
        );
        clamp(
            &mut w,
            "history.max_size_mb",
            &mut self.history.max_size_mb,
            0,
            1_048_576,
        );
        clamp(
            &mut w,
            "history.thumb_px",
            &mut self.history.thumb_px,
            32,
            1024,
        );
        clamp(
            &mut w,
            "about.check_every_days",
            &mut self.about.check_every_days,
            1,
            365,
        );
        clamp_usize(
            &mut w,
            "annotation.undo_limit",
            &mut self.annotation.undo_limit,
            10,
            5000,
        );

        for (key, value, fallback) in [
            (
                "annotation.stroke_color",
                &mut self.annotation.stroke_color,
                "#E81123",
            ),
            (
                "annotation.fill_color",
                &mut self.annotation.fill_color,
                "#00000000",
            ),
            ("appearance.accent", &mut self.appearance.accent, "#0078D7"),
            (
                "appearance.magnifier_foreground",
                &mut self.appearance.magnifier_foreground,
                "#FFFFFF",
            ),
            (
                "appearance.magnifier_background",
                &mut self.appearance.magnifier_background,
                "#1F1F1F",
            ),
        ] {
            if crate::colors::parse(value).is_none() {
                w.push(format!("{key}: {value} is not a colour, using {fallback}"));
                *value = fallback.to_string();
            }
        }
        let palette = std::mem::take(&mut self.annotation.palette);
        for c in palette {
            if crate::colors::parse(&c).is_some() {
                self.annotation.palette.push(c);
            } else {
                w.push(format!("annotation.palette: dropped {c}"));
            }
        }
        if self.annotation.palette.is_empty() {
            self.annotation.palette = default_palette();
        }

        if !self.annotation.default_tool.is_empty()
            && !ANNOTATION_TOOLS.contains(&self.annotation.default_tool.as_str())
        {
            w.push(format!(
                "annotation.default_tool: unknown {:?}, cleared",
                self.annotation.default_tool
            ));
            self.annotation.default_tool.clear();
        }
        if !matches!(
            self.annotation.arrow_style.as_str(),
            "straight" | "elbow" | "curve"
        ) {
            w.push(format!(
                "annotation.arrow_style: unknown {:?}, using straight",
                self.annotation.arrow_style
            ));
            self.annotation.arrow_style = "straight".into();
        }
        if !matches!(
            self.appearance.toolbar_placement.as_str(),
            "auto" | "top" | "bottom" | "left" | "right"
        ) {
            w.push(format!(
                "appearance.toolbar_placement: unknown {:?}, using auto",
                self.appearance.toolbar_placement
            ));
            self.appearance.toolbar_placement = "auto".into();
        }
        if !matches!(self.about.channel.as_str(), "stable" | "beta" | "nightly") {
            w.push(format!(
                "about.channel: unknown {:?}, using stable",
                self.about.channel
            ));
            self.about.channel = "stable".into();
        }
        if !matches!(
            self.advanced.log_level.as_str(),
            "off" | "error" | "warn" | "info" | "debug" | "trace"
        ) {
            w.push(format!(
                "advanced.log_level: unknown {:?}, using info",
                self.advanced.log_level
            ));
            self.advanced.log_level = "info".into();
        }
        if crate::colors::ColorFormat::from_name(&self.capture.magnifier_color_format).is_none() {
            w.push(format!(
                "capture.magnifier_color_format: unknown {:?}, using hex_upper",
                self.capture.magnifier_color_format
            ));
            self.capture.magnifier_color_format = "hex_upper".into();
        }
        if self.output.naming_template.is_empty() {
            w.push("output.naming_template: empty, using the default".to_string());
            self.output.naming_template = crate::naming::DEFAULT_TEMPLATE.into();
        } else if let Some(bad) = unknown_token(&self.output.naming_template) {
            w.push(format!(
                "output.naming_template: unknown %{} in {:?}, using the default",
                bad, self.output.naming_template
            ));
            self.output.naming_template = crate::naming::DEFAULT_TEMPLATE.into();
        }
        for (field, dir) in [
            ("output.save_dir", &mut self.output.save_dir),
            ("advanced.data_dir", &mut self.advanced.data_dir),
        ] {
            if !dir.is_empty() && !Path::new(dir.as_str()).is_absolute() {
                w.push(format!("{field}: {dir} must be absolute, ignored"));
                dir.clear();
            }
        }

        let bad_specs: Vec<String> = self
            .hotkey
            .actions
            .iter()
            .filter(|(_, spec)| {
                !spec.is_empty() && !spec.chars().all(|c| c.is_ascii_graphic() || c == ' ')
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in bad_specs {
            w.push(format!(
                "hotkey.actions.{id}: not a printable key spec, cleared"
            ));
            self.hotkey.actions.insert(id, String::new());
        }
        self.hotkey.exclusions.retain(|e| {
            let ok = !e.app.trim().is_empty();
            if !ok {
                w.push("hotkey.exclusions: dropped a rule with no app".to_string());
            }
            ok
        });

        let bad_corners: Vec<(String, String)> = self
            .hot_corner
            .corners
            .iter()
            .filter(|(_, action)| !action.is_empty() && !ACTIONS.contains(&action.as_str()))
            .map(|(corner, action)| (corner.clone(), action.clone()))
            .collect();
        for (corner, action) in bad_corners {
            w.push(format!(
                "hot_corner.corners.{corner}: unknown action {action:?}, cleared"
            ));
            self.hot_corner.corners.insert(corner, String::new());
        }
        w
    }
}

fn clamp(w: &mut Vec<String>, key: &str, v: &mut u32, lo: u32, hi: u32) {
    let c = (*v).clamp(lo, hi);
    if c != *v {
        w.push(format!("{key}: {v} is out of range, using {c}"));
        *v = c;
    }
}

fn clamp_usize(w: &mut Vec<String>, key: &str, v: &mut usize, lo: usize, hi: usize) {
    let c = (*v).clamp(lo, hi);
    if c != *v {
        w.push(format!("{key}: {v} is out of range, using {c}"));
        *v = c;
    }
}

/// The first `%x` the naming engine would emit verbatim, i.e. a typo.
fn unknown_token(template: &str) -> Option<char> {
    let mut it = template.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            continue;
        }
        match it.next() {
            None => return Some('%'),
            Some('%') => {}
            Some('Y' | 'y' | 'm' | 'd' | 'H' | 'M' | 'S' | 'p' | 'i' | 'w' | 'h' | 't' | 'B') => {}
            Some(other) => return Some(other),
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportChange {
    pub key: String,
    pub from: String,
    pub to: String,
    pub restart_required: bool,
}

/// §5.20.3 suggests a limited import range; `All` is the opt-in escape hatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportScope {
    /// Hotkeys, annotation styles, palette, theme, output directory and naming,
    /// group configuration.
    Suggested,
    All,
}

const SUGGESTED_PREFIXES: &[&str] = &[
    "hotkey.",
    "annotation.",
    "appearance.",
    "pin.",
    "output.save_dir",
    "output.naming_template",
    "output.collision",
    "output.format",
];

impl ImportScope {
    pub fn covers(self, key: &str) -> bool {
        match self {
            ImportScope::All => true,
            ImportScope::Suggested => SUGGESTED_PREFIXES
                .iter()
                .any(|p| key.starts_with(p) || key == p.trim_end_matches('.')),
        }
    }
}

fn flatten(tbl: &Table, prefix: &str, out: &mut BTreeMap<String, String>) {
    for (key, item) in tbl.iter() {
        let path = if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{prefix}.{key}")
        };
        match item {
            Item::Table(t) => flatten(t, &path, out),
            Item::Value(Value::InlineTable(t)) => flatten_inline(t, &path, out),
            Item::Value(v) => {
                out.insert(path, render(v));
            }
            _ => {}
        }
    }
}

fn flatten_inline(tbl: &InlineTable, prefix: &str, out: &mut BTreeMap<String, String>) {
    for (key, value) in tbl.iter() {
        let path = format!("{prefix}.{key}");
        match value {
            Value::InlineTable(t) => flatten_inline(t, &path, out),
            v => {
                out.insert(path, render(v));
            }
        }
    }
}

fn render(v: &Value) -> String {
    match v {
        Value::String(s) => s.value().to_string(),
        Value::Integer(i) => i.value().to_string(),
        Value::Float(f) => f.value().to_string(),
        Value::Boolean(b) => b.value().to_string(),
        Value::Datetime(d) => d.value().to_string(),
        Value::Array(a) => a.iter().map(render).collect::<Vec<_>>().join(", "),
        Value::InlineTable(t) => {
            let mut parts = Vec::new();
            for (k, v) in t.iter() {
                parts.push(format!("{k}={}", render(v)));
            }
            parts.join(", ")
        }
    }
}

/// Walk `section.[sub.]key`. A map such as `[hotkey.actions]` serialises as an
/// inline table, whose leaves are values rather than items. Paths in this
/// schema are never deeper than three segments.
fn find_value<'a>(doc: &'a DocumentMut, parts: &[&str]) -> Option<&'a Value> {
    if parts.len() < 2 {
        return None;
    }
    let head = doc.get(parts[0])?;
    if parts.len() == 2 {
        return match head {
            Item::Table(t) => t.get(parts[1])?.as_value(),
            Item::Value(Value::InlineTable(t)) => t.get(parts[1]),
            _ => None,
        };
    }
    let mid = head.as_table()?.get(parts[1])?;
    match mid {
        Item::Value(Value::InlineTable(t)) => t.get(parts[2]),
        Item::Table(t) => t.get(parts[2])?.as_value(),
        _ => None,
    }
}

fn find_value_mut<'a>(doc: &'a mut DocumentMut, parts: &[&str]) -> Option<&'a mut Value> {
    if parts.len() < 2 {
        return None;
    }
    let tail = parts[parts.len() - 1];
    if parts.len() == 2 {
        return match doc.get_mut(parts[0])? {
            Item::Table(t) => t.get_mut(tail)?.as_value_mut(),
            Item::Value(Value::InlineTable(t)) => t.get_mut(tail),
            _ => None,
        };
    }
    let mid = doc.get_mut(parts[0])?.as_table_mut()?.get_mut(parts[1])?;
    match mid {
        Item::Value(Value::InlineTable(t)) => t.get_mut(tail),
        Item::Table(t) => t.get_mut(tail)?.as_value_mut(),
        _ => None,
    }
}

/// The serialiser writes every struct it meets as one inline table, but a
/// configuration file is something a person opens in an editor: `[section]`
/// headers, one value per line. Hoist them back.
fn promote_tables(tbl: &mut Table) {
    let mut heads = Vec::new();
    for (key, item) in tbl.iter() {
        if let Item::Value(Value::InlineTable(inline)) = item {
            let mut table = Table::new();
            for (k, v) in inline.iter() {
                table.insert(k, Item::Value(v.clone()));
            }
            table.decor_mut().set_prefix(
                "
",
            );
            heads.push((key.to_string(), table));
        }
    }
    for (key, mut table) in heads {
        promote_tables(&mut table);
        if let Some(item) = tbl.get_mut(&key) {
            *item = Item::Table(table);
        }
    }
}

/// Copy every value from `src` into `dst`, keeping `dst`'s decor and comments.
/// Keys the schema no longer knows are left alone, not deleted.
fn merge_table(dst: &mut Table, src: &Table) {
    for (key, item) in src.iter() {
        match item {
            Item::Table(st) => {
                if let Some(Item::Table(dt)) = dst.get_mut(key) {
                    merge_table(dt, st);
                    continue;
                }
                dst.insert(key, Item::Table(st.clone()));
            }
            Item::Value(sv) => {
                if let Some(dv) = dst.get_mut(key).and_then(Item::as_value_mut) {
                    let decor = dv.decor().clone();
                    *dv = sv.clone();
                    dv.decor_mut().clone_from(&decor);
                    continue;
                }
                dst.insert(key, Item::Value(sv.clone()));
            }
            other => {
                dst.insert(key, other.clone());
            }
        }
    }
}

/// Parse `raw` into the type `old` already has.
fn coerce(old: &Value, leaf: &str, raw: &str) -> Result<Value, ConfigError> {
    let bad = |reason: String| ConfigError::InvalidValue {
        key: leaf.to_string(),
        reason,
    };
    Ok(match old {
        Value::Boolean(_) => match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Value::from(true),
            "0" | "false" | "no" | "off" | "" => Value::from(false),
            other => return Err(bad(format!("{other} is not a boolean"))),
        },
        Value::Integer(_) => raw
            .trim()
            .replace([' ', '_'], "")
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| bad(format!("{} is not a whole number", raw.trim())))?,
        Value::Float(_) => raw
            .trim()
            .parse::<f64>()
            .map(Value::from)
            .map_err(|_| bad(format!("{} is not a number", raw.trim())))?,
        Value::Array(_) => {
            let mut arr = Array::new();
            for part in raw.split(',') {
                let p = part.trim();
                if !p.is_empty() {
                    arr.push(p);
                }
            }
            Value::Array(arr)
        }
        Value::InlineTable(_) => return Err(bad("this setting is not a single value".into())),
        _ => Value::from(raw.to_string()),
    })
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    use std::io::Write;
    let mut f = atomic_write_file::AtomicWriteFile::open(path)
        .map_err(|e| ConfigError::Io(format!("{}: {e}", path.display())))?;
    f.as_file_mut()
        .write_all(bytes)
        .map_err(|e| ConfigError::Io(format!("{}: {e}", path.display())))?;
    // `commit` is fsync + rename, which is exactly the rule in plan §6.1.
    f.commit()
        .map_err(|e| ConfigError::Io(format!("{}: {e}", path.display())))?;
    Ok(())
}

/// `config.toml.broken-<unix seconds>`; a collision adds `_2`, `_3`, …
fn quarantine(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy().to_string();
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut i = 0u32;
    loop {
        let suffix = if i == 0 {
            format!("{name}.broken-{secs}")
        } else {
            format!("{name}.broken-{secs}_{i}")
        };
        let cand = path.with_file_name(suffix);
        if !cand.exists() {
            return match std::fs::rename(path, &cand) {
                Ok(()) => Some(cand),
                Err(_) => None,
            };
        }
        i += 1;
    }
}

/// Where `config.toml` lives: the OS config dir, unless `advanced.data_dir`
/// points somewhere else (§6.1).
pub fn config_path(cfg: &Config) -> PathBuf {
    if !cfg.advanced.data_dir.is_empty() {
        return Path::new(&cfg.advanced.data_dir).join(FILE_NAME);
    }
    directories::BaseDirs::new()
        .map(|d| d.config_dir().join("ai-falconshot").join(FILE_NAME))
        .unwrap_or_else(|| PathBuf::from(FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let c = Config::default();
        let text = c.export_text().unwrap();
        let back = Config::parse_import(&text).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn export_is_an_edited_by_hand_shaped_file() {
        // The serialiser's natural shape is one inline table per section, which
        // no one would keep in an editor. Writing must produce `[section]`
        // headers, and a nested map must become its own header too.
        let text = Config::default().export_text().unwrap();
        for header in [
            "[general]",
            "[capture]",
            "[annotation]",
            "[pin]",
            "[output]",
            "[hotkey]",
            "[hotkey.actions]",
            "[hot_corner]",
            "[hot_corner.corners]",
            "[appearance]",
            "[history]",
            "[advanced]",
            "[about]",
        ] {
            assert!(text.contains(header), "missing {header} in\n{text}");
        }
        assert!(
            text.contains("capture = \"ctrl+shift+a\""),
            "hotkey map lost its leaf\n{text}"
        );
        assert!(text.contains('\n'), "one flat line\n{text}");
    }

    #[test]
    fn every_category_is_readable_by_path() {
        let c = Config::default();
        let flat = c.flat().unwrap();
        for section in [
            "general.",
            "capture.",
            "annotation.",
            "pin.",
            "output.",
            "hotkey.",
            "hot_corner.",
            "appearance.",
            "history.",
            "advanced.",
            "about.",
        ] {
            assert!(
                flat.keys().any(|k| k.starts_with(section)),
                "missing section {section}"
            );
        }
        assert_eq!(c.get_str("capture.magnifier_zoom").as_deref(), Some("8"));
        assert_eq!(c.get_str("output.format").as_deref(), Some("png"));
        assert_eq!(c.get_str("general.show_tray").as_deref(), Some("true"));
        assert_eq!(
            c.get_str("output.naming_template").as_deref(),
            Some("%p_%Y-%m-%d_%H%M%S")
        );
        assert_eq!(
            c.get_str("hotkey.actions.capture").as_deref(),
            Some("ctrl+shift+a")
        );
        assert_eq!(
            c.get_str("hot_corner.corners.bottom_right").as_deref(),
            Some("capture")
        );
        assert_eq!(
            c.get_str("annotation.palette").as_deref(),
            Some("#E81123, #F38B00, #FFB900, #7A7A7A, #FFFFFF, #000000, #0078D7, #16C60C, #B4009E, #00B7C3")
        );
        assert!(c.has_key("history.max_items"));
        assert!(!c.has_key("history.nope"));
        assert!(!c.has_key("history"));
    }

    #[test]
    fn a_partial_file_fills_in_from_defaults() {
        let c: Config = toml_edit::de::from_str(
            r#"
[capture]
magnifier_zoom = 12
"#,
        )
        .unwrap();
        assert_eq!(c.capture.magnifier_zoom, 12);
        assert_eq!(c.capture.magnifier_radius, 8);
        assert!(c.general.show_tray);
        assert_eq!(c.hotkey.actions["capture"], "ctrl+shift+a");
    }

    #[test]
    fn set_str_keeps_the_stored_type() {
        let mut c = Config::default();
        c.set_str("capture.magnifier_zoom", "16").unwrap();
        assert_eq!(c.capture.magnifier_zoom, 16);
        c.set_str("general.show_tray", "off").unwrap();
        assert!(!c.general.show_tray);
        c.set_str("output.format", "jpg").unwrap();
        assert_eq!(c.output.format, Format::Jpg);
        c.set_str("output.save_dir", "D:/shots").unwrap();
        assert_eq!(c.output.save_dir, "D:/shots");
        // A thousands separator from a spin box still means a number.
        c.set_str("history.max_items", "1 000").unwrap();
        assert_eq!(c.history.max_items, 1000);
        // Nested maps and lists are reachable through the same path API.
        c.set_str("hotkey.actions.capture", "f13").unwrap();
        assert_eq!(c.hotkey.actions["capture"], "f13");
        c.set_str("annotation.palette", "#FF0000, #00FF00").unwrap();
        assert_eq!(c.annotation.palette, ["#FF0000", "#00FF00"]);
        assert!(c.set_str("capture.magnifier_zoom", "big").is_err());
        assert!(c.set_str("general.show_tray", "maybe").is_err());
        assert!(c.set_str("nope", "1").is_err());
        assert!(c.set_str("capture", "1").is_err());
        assert!(c.set_str("capture.nope", "1").is_err());
        assert!(c.set_str("capture.magnifier.", "1").is_err());
        // A rejected write leaves the config untouched.
        assert_eq!(c.capture.magnifier_zoom, 16);
    }

    #[test]
    fn validate_fixes_values_and_says_so() {
        let mut c = Config::default();
        c.pin.default_opacity = 900;
        c.annotation.arrow_style = "spiral".into();
        c.annotation.palette = ["#FFFFFF", "not-a-colour"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        c.annotation.stroke_color = "reddish".into();
        c.output.jpeg_quality = 0;
        c.output.naming_template = "%p_%Q".into();
        c.capture.magnifier_color_format = "crayon".into();
        c.output.save_dir = "relative/dir".into();
        c.hot_corner
            .corners
            .insert("top_left".into(), "nope".into());
        let warnings = c.validate();
        assert_eq!(c.pin.default_opacity, 100);
        assert_eq!(c.annotation.arrow_style, "straight");
        assert_eq!(c.annotation.palette, ["#FFFFFF"]);
        assert_eq!(c.annotation.stroke_color, "#E81123");
        assert_eq!(c.output.jpeg_quality, 1);
        assert_eq!(c.output.naming_template, crate::naming::DEFAULT_TEMPLATE);
        assert_eq!(c.capture.magnifier_color_format, "hex_upper");
        assert!(c.output.save_dir.is_empty());
        assert_eq!(c.hot_corner.corners["top_left"], "");
        for needle in [
            "pin.default_opacity",
            "arrow_style",
            "palette",
            "stroke_color",
            "jpeg_quality",
            "naming_template",
            "magnifier_color_format",
            "save_dir",
            "hot_corner.corners.top_left",
        ] {
            assert!(
                warnings.iter().any(|w| w.contains(needle)),
                "no warning for {needle}: {warnings:?}"
            );
        }
        // The defaults need no repair at all.
        assert!(Config::default().validate().is_empty());
        // A palette of nothing comes back rather than an empty swatch row.
        let mut empty = Config::default();
        empty.annotation.palette.clear();
        assert!(empty.validate().is_empty());
        assert_eq!(empty.annotation.palette, default_palette());
    }

    #[test]
    fn save_rewrites_values_and_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(
            &path,
            "# my settings\n[capture]\n# the zoom I like\nmagnifier_zoom = 4  # four\n\n[output]\nformat = \"bmp\"\n",
        )
        .unwrap();
        let (mut c, report) = Config::load(&path);
        assert_eq!(report.source, LoadSource::File);
        assert_eq!(c.capture.magnifier_zoom, 4);
        assert_eq!(c.output.format, Format::Bmp);
        c.capture.magnifier_zoom = 9;
        c.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my settings"), "{text}");
        assert!(text.contains("the zoom I like"), "{text}");
        assert!(text.contains("four"), "{text}");
        assert!(text.contains("magnifier_zoom = 9"), "{text}");
        // Sections the user never wrote are appended on save.
        assert!(text.contains("freeze_frame"), "{text}");
        let (again, _) = Config::load(&path);
        assert_eq!(again.capture.magnifier_zoom, 9);
        assert_eq!(again.output.format, Format::Bmp);
        assert_eq!(again.capture.magnifier_radius, 8);
    }

    #[test]
    fn a_damaged_file_is_kept_and_defaults_win() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "[capture\n  ??? not toml").unwrap();
        let (c, report) = Config::load(&path);
        assert_eq!(report.source, LoadSource::Recovered);
        let backup = report.backup.clone().expect("a backup path");
        assert!(backup.exists());
        assert!(!path.exists(), "the damaged file must be moved aside");
        assert!(backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("config.toml.broken-"));
        assert_eq!(c, Config::default());
        // The next save writes a clean file.
        c.save(&path).unwrap();
        let (c2, r2) = Config::load(&path);
        assert_eq!(r2.source, LoadSource::File);
        assert_eq!(c2, c);
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join(FILE_NAME);
        let (c, report) = Config::load(&path);
        assert_eq!(report.source, LoadSource::Defaults);
        assert_eq!(c, Config::default());
        c.save(&path).unwrap();
        assert!(path.exists(), "save creates the parent directory");
    }

    #[test]
    fn import_preview_only_lists_what_changes() {
        let mut incoming = Config::default();
        incoming
            .hotkey
            .actions
            .insert("capture".into(), "f13".into());
        incoming.general.language = "en".into();
        let current = Config::default();
        let changes = current.preview_import(&incoming, ImportScope::All);
        let keys: Vec<&str> = changes.iter().map(|c| c.key.as_str()).collect();
        assert!(keys.contains(&"hotkey.actions.capture"), "{keys:?}");
        assert!(keys.contains(&"general.language"), "{keys:?}");
        let language = changes
            .iter()
            .find(|c| c.key == "general.language")
            .unwrap();
        assert!(language.restart_required, "language needs a restart");
        let hotkey = changes
            .iter()
            .find(|c| c.key == "hotkey.actions.capture")
            .unwrap();
        assert!(!hotkey.restart_required);
        assert_eq!(hotkey.from, "ctrl+shift+a");
        assert_eq!(hotkey.to, "f13");

        // The suggested range from §5.20.3 does not touch general settings.
        let scoped = current.preview_import(&incoming, ImportScope::Suggested);
        assert_eq!(scoped.len(), 1, "{scoped:?}");
        assert_eq!(scoped[0].key, "hotkey.actions.capture");
        assert!(ImportScope::Suggested.covers("annotation.palette"));
        assert!(ImportScope::Suggested.covers("output.naming_template"));
        assert!(ImportScope::Suggested.covers("appearance.theme"));
        assert!(!ImportScope::Suggested.covers("history.max_items"));
        assert!(!ImportScope::Suggested.covers("general.show_tray"));

        let mut applied = current.clone();
        applied
            .apply_import(&incoming, ImportScope::Suggested)
            .unwrap();
        assert_eq!(applied.hotkey.actions["capture"], "f13");
        assert_eq!(applied.general.language, "auto", "out of scope stays put");
        applied.apply_import(&incoming, ImportScope::All).unwrap();
        assert_eq!(applied.general.language, "en");
        // Importing yourself changes nothing.
        assert!(current
            .preview_import(&current, ImportScope::All)
            .is_empty());
    }

    #[test]
    fn hotkey_exclusions_keep_their_shape() {
        let text = r#"
[hotkey]
actions = { capture = "ctrl+shift+a" }

[[hotkey.exclusions]]
app = "game.exe"
actions = [ "capture" ]

[[hotkey.exclusions]]
app = "C:/Apps/presentation.exe"
actions = []
"#;
        let c: Config = toml_edit::de::from_str(text).unwrap();
        assert_eq!(c.hotkey.exclusions.len(), 2);
        assert_eq!(c.hotkey.exclusions[0].app, "game.exe");
        assert_eq!(c.hotkey.exclusions[0].actions, ["capture"]);
        assert!(c.hotkey.exclusions[1].actions.is_empty());
        // A rule is a table, so the text path refuses to scribble over it, and a
        // rule with no app is dropped instead of matching everything.
        let mut c2 = c.clone();
        assert!(c2.set_str("hotkey.exclusions", "x").is_err());
        c2.hotkey.exclusions.push(AppExclusion::default());
        let warnings = c2.validate();
        assert!(
            warnings.iter().any(|w| w.contains("exclusions")),
            "{warnings:?}"
        );
        assert_eq!(c2.hotkey.exclusions.len(), 2);
    }

    #[test]
    fn save_is_atomic_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut c = Config::default();
        for i in 0..5 {
            c.capture.magnifier_zoom = 2 + i;
            c.save(&path).unwrap();
        }
        assert_eq!(c.capture.magnifier_zoom, 6);
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n != FILE_NAME)
            .collect();
        assert!(left.is_empty(), "stray temp files: {left:?}");
        let (back, _) = Config::load(&path);
        assert_eq!(back.capture.magnifier_zoom, 6);
    }

    #[test]
    fn naming_template_tokens_are_the_ones_the_engine_knows() {
        assert_eq!(unknown_token("%p_%Y-%m-%d_%H%M%S_%i_%w_%h_%t_%B_%%"), None);
        assert_eq!(unknown_token("%Q"), Some('Q'));
        assert_eq!(unknown_token("%"), Some('%'));
        assert_eq!(unknown_token("no tokens"), None);
    }
}
