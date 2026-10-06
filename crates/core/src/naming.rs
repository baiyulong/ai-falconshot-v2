//! File names for auto-save, quick-save and the CLI (PRD §5.5.3, §5.5.4, §5.17).
//!
//! The template is strftime-style so it is familiar and because a hand-rolled
//! `{date}/{time}` pair collides on `mm`. The caller supplies the wall-clock
//! reading: core has no timezone database and `GetLocalTime` belongs to the
//! platform layer.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalStamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub min: u8,
    pub sec: u8,
}

impl LocalStamp {
    /// Whole seconds since the Unix epoch plus a minutes-east-of-UTC offset.
    pub fn from_epoch(epoch_s: i64, offset_min: i32) -> Self {
        let days = epoch_s.div_euclid(86_400);
        let sec_of_day = epoch_s.rem_euclid(86_400) + offset_min as i64 * 60;
        // Normalise after adding the offset: it can cross a day boundary.
        let days = days + sec_of_day.div_euclid(86_400);
        let sec_of_day = sec_of_day.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        Self {
            year: y as u16,
            month: m,
            day: d,
            hour: (sec_of_day / 3600) as u8,
            min: ((sec_of_day % 3600) / 60) as u8,
            sec: (sec_of_day % 60) as u8,
        }
    }
}

/// Days since 1970-01-01 to a proleptic-Gregorian date (Howard Hinnant's
/// `civil_from_days`, shifted to a 0000-03-01 epoch so the leap day is last).
fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u8, d as u8)
}

/// What the template can refer to beyond the clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct NameFacts<'a> {
    pub width: u32,
    pub height: u32,
    pub monitor: &'a str,
    pub tool: &'a str,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Collision {
    /// `_1`, `_2`, … until the name is free (PRD §5.5.3).
    #[default]
    Increment,
    /// Clobber whatever is there.
    Overwrite,
    /// Refuse, so the caller can tell the user (§8.3).
    KeepBoth,
}

pub const DEFAULT_TEMPLATE: &str = "%p_%Y-%m-%d_%H%M%S";
pub const MAX_BASE_LEN: usize = 120;

/// Expand `%Y %y %m %d %H %M %S %p %i %w %h %t %B %%`. Unknown sequences are
/// kept verbatim rather than dropped, so a typo in the settings page is visible.
pub fn expand(
    template: &str,
    stamp: LocalStamp,
    prefix: &str,
    seq: u32,
    facts: NameFacts<'_>,
) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&format!("{:04}", stamp.year)),
            Some('y') => out.push_str(&format!("{:02}", stamp.year % 100)),
            Some('m') => out.push_str(&format!("{:02}", stamp.month)),
            Some('d') => out.push_str(&format!("{:02}", stamp.day)),
            Some('H') => out.push_str(&format!("{:02}", stamp.hour)),
            Some('M') => out.push_str(&format!("{:02}", stamp.min)),
            Some('S') => out.push_str(&format!("{:02}", stamp.sec)),
            Some('p') => out.push_str(prefix),
            Some('i') => out.push_str(&format!("{seq:03}")),
            Some('w') => out.push_str(&format!("{}", facts.width)),
            Some('h') => out.push_str(&format!("{}", facts.height)),
            Some('t') => out.push_str(facts.tool),
            Some('B') => out.push_str(facts.monitor),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Windows forbids these; every other OS gets them too, so one rule applies
/// everywhere and a config exported from Windows stays importable (§5.20.3).
pub fn sanitize(raw: &str) -> String {
    let replaced: String = raw
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    // Trailing dots and spaces are silently dropped by the shell.
    let s = replaced.trim().trim_end_matches(['.', ' ']).trim_end();
    if s.is_empty() || s.chars().all(|c| c == '_' || c == '.') {
        return "snip".to_string();
    }
    // Cap the base so the extension always survives (NTFS caps at 255 UTF-16).
    if s.chars().count() > MAX_BASE_LEN {
        return s.chars().take(MAX_BASE_LEN).collect();
    }
    s.to_string()
}

/// The name that should be written: template → sanitized base → extension.
pub fn file_name(
    template: &str,
    stamp: LocalStamp,
    prefix: &str,
    seq: u32,
    facts: NameFacts<'_>,
    ext: &str,
) -> String {
    let base = sanitize(&expand(template, stamp, prefix, seq, facts));
    if ext.is_empty() {
        base
    } else {
        format!("{base}.{}", ext.trim_start_matches('.'))
    }
}

/// Sequence suffix for a collision: `name`, then `name_1`, `name_2`, …
fn with_suffix(name: &str, seq: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => {
            let (stem, ext) = name.split_at(dot);
            format!("{stem}_{seq}{}", ext)
        }
        _ => format!("{name}_{seq}"),
    }
}

pub trait Exists {
    fn exists(&self, path: &Path) -> bool;
}

impl<F: Fn(&Path) -> bool> Exists for F {
    fn exists(&self, path: &Path) -> bool {
        (self)(path)
    }
}

/// Pick the path to write. `dir` is created by the caller (`encode::save`
/// creates it too), so this stays a pure name decision and is testable without
/// touching the disk.
pub fn resolve<E: Exists>(
    dir: &Path,
    name: &str,
    collision: &Collision,
    exists: E,
) -> Option<PathBuf> {
    let first = dir.join(name);
    if !exists.exists(&first) {
        return Some(first);
    }
    match collision {
        Collision::Overwrite => Some(first),
        Collision::KeepBoth => None,
        Collision::Increment => {
            for seq in 1..=9_999u32 {
                let candidate = dir.join(with_suffix(name, seq));
                if !exists.exists(&candidate) {
                    return Some(candidate);
                }
            }
            None
        }
    }
}

/// Real-filesystem variant used by the save paths.
pub fn resolve_on_disk(dir: &Path, name: &str, collision: &Collision) -> Option<PathBuf> {
    resolve(dir, name, collision, |p: &Path| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> LocalStamp {
        LocalStamp {
            year: 2026,
            month: 10,
            day: 6,
            hour: 7,
            min: 8,
            sec: 9,
        }
    }

    #[test]
    fn template_expansion() {
        let n = file_name(
            DEFAULT_TEMPLATE,
            stamp(),
            "falcon",
            1,
            NameFacts::default(),
            "png",
        );
        assert_eq!(n, "falcon_2026-10-06_070809.png");
        let n = file_name(
            "%t-%B-%wx%h-#%i",
            stamp(),
            "p",
            7,
            NameFacts {
                width: 1024,
                height: 640,
                monitor: "main",
                tool: "rect",
            },
            "jpg",
        );
        assert_eq!(n, "rect-main-1024x640-#007.jpg");
        // Unknown sequences survive so a typo is visible instead of vanishing.
        assert_eq!(
            expand("%Q%M", stamp(), "p", 0, NameFacts::default()),
            "%Q08"
        );
        assert_eq!(
            expand("100%%", stamp(), "p", 0, NameFacts::default()),
            "100%"
        );
    }

    #[test]
    fn epoch_converts_with_offset_across_the_day_boundary() {
        // 2026-10-06T00:00:00Z.
        const EPOCH: i64 = 20_732 * 86_400;
        let utc = LocalStamp::from_epoch(EPOCH, 0);
        assert_eq!(
            (utc.year, utc.month, utc.day, utc.hour, utc.min, utc.sec),
            (2026, 10, 6, 0, 0, 0)
        );
        assert_eq!(LocalStamp::from_epoch(EPOCH, 480).day, 6);
        assert_eq!(LocalStamp::from_epoch(EPOCH, 480).hour, 8);
        assert_eq!(LocalStamp::from_epoch(EPOCH, -300).day, 5);
        assert_eq!(LocalStamp::from_epoch(EPOCH, -300).hour, 19);
        assert_eq!(LocalStamp::from_epoch(EPOCH, -660).year, 2026);
        assert_eq!(LocalStamp::from_epoch(EPOCH, -660).month, 10);
        assert_eq!(LocalStamp::from_epoch(EPOCH, -660).day, 5);
    }

    #[test]
    fn civil_conversion_matches_known_dates() {
        let cases = [
            (0i64, (1970u16, 1u8, 1u8)),
            (-1, (1969, 12, 31)),
            (19_782, (2024, 2, 29)),
            (19_783, (2024, 3, 1)),
            (20_732, (2026, 10, 6)),
            (24_105, (2035, 12, 31)),
            (24_106, (2036, 1, 1)),
        ];
        for (days, want) in cases {
            let (y, m, d) = civil_from_days(days);
            assert_eq!((y as u16, m, d), want, "day {days}");
        }
    }

    #[test]
    fn illegal_characters_are_replaced_not_propagated() {
        assert_eq!(sanitize("a<b>c:d\"e/f\\g|h?i*j"), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(sanitize("  spaced  ."), "spaced");
        assert_eq!(sanitize("***"), "snip");
        assert_eq!(sanitize("   "), "snip");
        assert_eq!(sanitize("\u{1}control"), "_control");
        let long = sanitize(&"x".repeat(500));
        assert_eq!(long.chars().count(), MAX_BASE_LEN);
        // A path never escapes the directory: the separator became an underscore.
        assert_eq!(sanitize("../evil"), ".._evil");
        assert!(!sanitize("../evil").contains('/'));
    }

    #[test]
    fn collision_modes() {
        let dir = Path::new("C:/out");
        let mut taken = std::collections::HashSet::new();
        taken.insert("a_2026.png".to_string());
        taken.insert("a_2026_1.png".to_string());
        let probe = |p: &Path| {
            p.file_name()
                .map(|n| taken.contains(&n.to_string_lossy().to_string()))
                .unwrap_or(false)
        };
        assert_eq!(
            resolve(dir, "a_2026.png", &Collision::Increment, probe).unwrap(),
            dir.join("a_2026_2.png")
        );
        assert_eq!(
            resolve(dir, "x.png", &Collision::Overwrite, |_: &Path| true).unwrap(),
            dir.join("x.png")
        );
        assert_eq!(
            resolve(dir, "x.png", &Collision::KeepBoth, |_: &Path| true),
            None
        );
        // Free name short-circuits every mode.
        assert_eq!(
            resolve(dir, "x.png", &Collision::KeepBoth, |_: &Path| false).unwrap(),
            dir.join("x.png")
        );
        // A template that ignores %i still lands on a distinct file (§5.5.3).
        let name = file_name("%p", stamp(), "same", 1, NameFacts::default(), "png");
        assert_eq!(name, "same.png");
        let base_only = |p: &Path| p.file_name() == Some(std::ffi::OsStr::new("same.png"));
        assert_eq!(
            resolve(dir, &name, &Collision::Increment, base_only).unwrap(),
            dir.join("same_1.png")
        );
    }

    #[test]
    fn suffix_inserts_before_the_extension() {
        assert_eq!(with_suffix("no ext", 3), "no ext_3");
        assert_eq!(with_suffix("a.png", 3), "a_3.png");
        assert_eq!(with_suffix(".hidden", 1), ".hidden_1");
    }
}
