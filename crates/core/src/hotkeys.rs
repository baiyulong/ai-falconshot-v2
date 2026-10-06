//! Key specs for the global hotkeys (PRD §5.16): parse what the user typed or
//! what the recorder produced, canonicalise it so `Ctrl+Shift+A` and
//! `shift+control+a` are the same binding, find conflicts before they are
//! saved (§8.4), and decide whether the foreground app suppresses a binding
//! (§5.16.2).
//!
//! Nothing here touches Win32. Turning a [`Key`] into a virtual-key code is
//! `platform-windows`' job; keeping the grammar here is what lets the settings
//! page be tested without a desktop.

use crate::config::AppExclusion;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Keys that have no printable character of their own. Punctuation that does
/// (`-`, `=`, `[`) stays a [`Key::Char`] so one grammar covers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Special {
    Escape,
    Tab,
    Enter,
    Space,
    Backspace,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    PrintScreen,
    ScrollLock,
    NumLock,
    CapsLock,
    Pause,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    /// A letter, digit or punctuation character, always stored lowercase.
    Char(char),
    /// `f1` … `f24`.
    Func(u8),
    Special(Special),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Windows key. `super`/`meta`/`cmd` are aliases for it.
    pub win: bool,
}

impl Modifiers {
    pub fn none(&self) -> bool {
        !(self.ctrl || self.alt || self.shift || self.win)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Spec {
    pub mods: Modifiers,
    pub key: Key,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HotkeyError {
    #[error("empty")]
    Empty,
    #[error("unknown token {0:?}")]
    UnknownToken(String),
    #[error("F{0} does not exist")]
    BadFunction(u8),
    #[error("{0} has no modifiers, and Windows will not deliver it globally")]
    NeedsModifier(String),
    /// Not a key we know at all (an emoji, a combining mark, a surrogate pair).
    #[error("unsupported key {0:?}")]
    UnsupportedKey(String),
}

/// Combinations the shell takes before `RegisterHotKey` ever sees them, so a
/// conflict report naming another app would be misleading.
pub const RESERVED: &[(Modifiers, Key, &str)] = &[
    (
        Modifiers {
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
        },
        Key::Special(Special::Delete),
        "secure attention sequence",
    ),
    (
        Modifiers {
            ctrl: false,
            alt: false,
            shift: false,
            win: true,
        },
        Key::Char('l'),
        "locks the session",
    ),
    (
        Modifiers {
            ctrl: false,
            alt: false,
            shift: false,
            win: true,
        },
        Key::Char('d'),
        "shows the desktop",
    ),
    (
        Modifiers {
            ctrl: false,
            alt: false,
            shift: false,
            win: true,
        },
        Key::Special(Special::Tab),
        "opens task view",
    ),
    (
        Modifiers {
            ctrl: false,
            alt: false,
            shift: false,
            win: true,
        },
        Key::Char('u'),
        "is reserved for the accessibility launcher",
    ),
];

fn key_label(key: Key) -> String {
    match key {
        Key::Char(c) => c.to_string(),
        Key::Func(n) => format!("f{n}"),
        Key::Special(s) => match s {
            Special::Escape => "escape",
            Special::Tab => "tab",
            Special::Enter => "enter",
            Special::Space => "space",
            Special::Backspace => "backspace",
            Special::Insert => "insert",
            Special::Delete => "delete",
            Special::Home => "home",
            Special::End => "end",
            Special::PageUp => "pageup",
            Special::PageDown => "pagedown",
            Special::Up => "up",
            Special::Down => "down",
            Special::Left => "left",
            Special::Right => "right",
            Special::PrintScreen => "print",
            Special::ScrollLock => "scrolllock",
            Special::NumLock => "numlock",
            Special::CapsLock => "capslock",
            Special::Pause => "pause",
        }
        .to_string(),
    }
}

/// The label a settings page shows. Modifier order is the one Windows writes in
/// its own hotkey dialogs.
pub fn display(spec: Spec) -> String {
    let mut parts: Vec<String> = Vec::new();
    if spec.mods.ctrl {
        parts.push("Ctrl".into());
    }
    if spec.mods.alt {
        parts.push("Alt".into());
    }
    if spec.mods.shift {
        parts.push("Shift".into());
    }
    if spec.mods.win {
        parts.push("Win".into());
    }
    parts.push(match spec.key {
        Key::Char(c) => c.to_uppercase().to_string(),
        Key::Func(n) => format!("F{n}"),
        Key::Special(s) => special_display(s).to_string(),
    });
    parts.join("+")
}

impl Spec {
    /// The form stored in `config.toml`: lowercase, modifiers in a fixed order,
    /// so a rebind that only changes capitalisation still compares equal.
    pub fn canonical(&self) -> String {
        let mut out = String::new();
        for (on, name) in [
            (self.mods.ctrl, "ctrl"),
            (self.mods.alt, "alt"),
            (self.mods.shift, "shift"),
            (self.mods.win, "win"),
        ] {
            if on {
                out.push_str(name);
                out.push('+');
            }
        }
        out.push_str(&key_label(self.key));
        out
    }

    /// A binding with no modifiers is only global for keys no other app uses as
    /// text input: `Print` and the extended function keys.
    pub fn is_bindable(&self) -> bool {
        if !self.mods.none() {
            return true;
        }
        match self.key {
            Key::Func(n) => n >= 13,
            Key::Special(Special::PrintScreen) => true,
            _ => false,
        }
    }

    /// Why the shell would swallow this combination before we get it.
    pub fn reserved_reason(&self) -> Option<&'static str> {
        RESERVED
            .iter()
            .find(|(m, k, _)| *m == self.mods && *k == self.key)
            .map(|(_, _, why)| *why)
    }
}

fn parse_modifier(token: &str, mods: &mut Modifiers) -> Result<(), HotkeyError> {
    match token {
        "ctrl" | "control" => mods.ctrl = true,
        "alt" | "option" | "menu" => mods.alt = true,
        "shift" => mods.shift = true,
        "win" | "windows" | "super" | "meta" | "cmd" | "command" => mods.win = true,
        other => return Err(HotkeyError::UnknownToken(other.to_string())),
    }
    Ok(())
}

fn parse_key(token: &str) -> Result<Key, HotkeyError> {
    if token.is_empty() {
        return Err(HotkeyError::Empty);
    }
    if let Some(rest) = token.strip_prefix('f') {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            let n: u8 = rest
                .parse()
                .map_err(|_| HotkeyError::UnknownToken(token.to_string()))?;
            return match n {
                1..=24 => Ok(Key::Func(n)),
                other => Err(HotkeyError::BadFunction(other)),
            };
        }
    }
    let named = match token {
        "esc" | "escape" => Some(Special::Escape),
        "tab" => Some(Special::Tab),
        "enter" | "return" | "cr" => Some(Special::Enter),
        "space" | "spacebar" => Some(Special::Space),
        "backspace" | "bs" => Some(Special::Backspace),
        "ins" | "insert" => Some(Special::Insert),
        "del" | "delete" => Some(Special::Delete),
        "home" => Some(Special::Home),
        "end" => Some(Special::End),
        "pgup" | "pageup" => Some(Special::PageUp),
        "pgdn" | "pgdown" | "pagedown" => Some(Special::PageDown),
        "up" | "arrowup" => Some(Special::Up),
        "down" | "arrowdown" => Some(Special::Down),
        "left" | "arrowleft" => Some(Special::Left),
        "right" | "arrowright" => Some(Special::Right),
        "print" | "prnt" | "prntsc" | "prtsc" | "prtscn" | "printscreen" | "snapshot"
        | "pausescr" => Some(Special::PrintScreen),
        "scrolllock" | "scroll" => Some(Special::ScrollLock),
        "numlock" => Some(Special::NumLock),
        "capslock" | "caps" => Some(Special::CapsLock),
        "pause" | "break" => Some(Special::Pause),
        _ => None,
    };
    if let Some(s) = named {
        return Ok(Key::Special(s));
    }
    let mut chars = token.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Ok(Key::Char(c.to_ascii_lowercase())),
        (Some(_), Some(_)) => Err(HotkeyError::UnknownToken(token.to_string())),
        _ => Err(HotkeyError::Empty),
    }
}

/// What a settings page shows for one key. Each of these must also be a
/// [`parse`] alias, or the label stops being re-bindable.
fn special_display(key: Special) -> &'static str {
    match key {
        Special::Escape => "Esc",
        Special::Tab => "Tab",
        Special::Enter => "Enter",
        Special::Space => "Space",
        Special::Backspace => "Backspace",
        Special::Insert => "Ins",
        Special::Delete => "Del",
        Special::Home => "Home",
        Special::End => "End",
        Special::PageUp => "PageUp",
        Special::PageDown => "PageDown",
        Special::Up => "Up",
        Special::Down => "Down",
        Special::Left => "Left",
        Special::Right => "Right",
        Special::PrintScreen => "PrtScn",
        Special::ScrollLock => "ScrollLock",
        Special::NumLock => "NumLock",
        Special::CapsLock => "CapsLock",
        Special::Pause => "Pause",
    }
}

/// `ctrl+shift+a`, `Print`, `Win+Shift+S` … Anything the recorder emits must
/// come back through here unchanged, so the two agree on the grammar.
pub fn parse(input: &str) -> Result<Spec, HotkeyError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(HotkeyError::Empty);
    }
    // Only `+` separates, and the key is whatever follows the last one. The one
    // combination that needs care is the plus key itself: `ctrl++` is Ctrl and
    // Plus, while a lone trailing `+` (`ctrl+`) is a truncated spec, not a key.
    let (head, tail) = match text.rsplit_once('+') {
        Some((h, "")) if h.ends_with('+') => (&h[..h.len() - 1], "+"),
        Some((h, t)) => (h, t),
        None => ("", text),
    };
    let mut mods = Modifiers::default();
    for token in head.split('+') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        parse_modifier(&token.to_ascii_lowercase(), &mut mods)?;
    }
    let key = parse_key(tail.trim().to_ascii_lowercase().as_str())?;
    Ok(Spec { mods, key })
}

/// Parse and reject a combination a global registration could not deliver.
pub fn parse_bindable(input: &str) -> Result<Spec, HotkeyError> {
    let spec = parse(input)?;
    if !spec.is_bindable() {
        return Err(HotkeyError::NeedsModifier(display(spec)));
    }
    Ok(spec)
}

/// Two bindings that mean the same thing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub canonical: String,
    pub display: String,
    /// Action ids already using it, in config order.
    pub actions: Vec<String>,
    /// Set when the shell, not another action, owns the combination.
    pub reserved: Option<&'static str>,
}

/// §8.4: the report names the combination *and* the features fighting over it,
/// because "it is already taken" is not actionable.
pub fn conflicts(actions: &BTreeMap<String, String>) -> Vec<Conflict> {
    let mut by_spec: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, spec) in actions {
        if spec.trim().is_empty() {
            continue; // unbound
        }
        let canonical = match parse(spec) {
            Ok(s) => s.canonical(),
            // Invalid text gets its own report; do not merge it into a conflict.
            Err(_) => continue,
        };
        by_spec.entry(canonical).or_default().push(id.clone());
    }
    let mut out = Vec::new();
    for (canonical, mut ids) in by_spec {
        ids.sort();
        let spec = parse(&canonical).ok();
        let shared = ids.len() > 1;
        let reserved = spec.as_ref().and_then(|s| s.reserved_reason());
        if !shared && reserved.is_none() {
            continue;
        }
        out.push(Conflict {
            display: spec.map(display).unwrap_or_else(|| canonical.clone()),
            canonical,
            actions: ids,
            reserved,
        });
    }
    out.sort_by(|a, b| a.canonical.cmp(&b.canonical));
    out
}

/// Specs that will never register, so the settings page can refuse to save
/// rather than fail silently at startup (PRD line "全局快捷键注册失败时，应显示冲突提示").
pub fn invalid(actions: &BTreeMap<String, String>) -> Vec<(String, String, String)> {
    actions
        .iter()
        .filter_map(|(id, spec)| {
            if spec.trim().is_empty() {
                return None;
            }
            match parse_bindable(spec) {
                Ok(s) => s
                    .reserved_reason()
                    .map(|why| (id.clone(), s.canonical(), why.to_string())),
                Err(e) => Some((id.clone(), spec.clone(), e.to_string())),
            }
        })
        .collect()
}

fn normalise_path(text: &str) -> String {
    text.trim()
        .to_ascii_lowercase()
        .replace('\\', "/")
        .replace("//", "/")
}

/// `*` and `?` over the whole path or its file name, case- and separator-blind,
/// so `GAME.EXE` in the config matches `C:\Games\game.exe` (§5.16.2).
pub fn app_matches(pattern: &str, foreground: &str) -> bool {
    let pat = normalise_path(pattern);
    let text = normalise_path(foreground);
    if pat.is_empty() || text.is_empty() {
        return false;
    }
    let file = text.rsplit('/').next().unwrap_or(&text);
    wildmatch(&pat, &text) || wildmatch(&pat, file)
}

fn wildmatch(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut star_ti) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            pi += 1;
            star_ti = ti;
        } else if let Some(sp) = star {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Does the foreground application swallow `action`? An exclusion with no
/// listed actions covers every hotkey, which is the "只对指定快捷键" toggle's off
/// state.
pub fn is_suppressed(exclusions: &[AppExclusion], foreground: &str, action: &str) -> bool {
    exclusions.iter().any(|rule| {
        app_matches(&rule.app, foreground)
            && (rule.actions.is_empty() || rule.actions.iter().any(|a| a == action))
    })
}

/// The keys the recorder needs to know are already bound, for "press a key" UI.
pub fn bound_actions(actions: &BTreeMap<String, String>) -> Vec<(String, Spec)> {
    actions
        .iter()
        .filter_map(|(id, spec)| parse(spec).ok().map(|s| (id.clone(), s)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_binding_written_four_ways_is_one_binding() {
        for text in [
            "Ctrl+Shift+A",
            "shift+control+a",
            "CTRL+SHIFT+a",
            " ctrl + shift + a ",
        ] {
            let spec = parse(text).unwrap();
            assert_eq!(spec.canonical(), "ctrl+shift+a", "{text}");
        }
        assert_eq!(display(parse("ctrl+shift+a").unwrap()), "Ctrl+Shift+A");
    }

    #[test]
    fn single_keys_and_punctuation_survive_the_grammar() {
        assert_eq!(
            parse("print").unwrap().key,
            Key::Special(Special::PrintScreen)
        );
        assert_eq!(parse("F13").unwrap().key, Key::Func(13));
        assert_eq!(parse("ctrl++").unwrap().canonical(), "ctrl++");
        assert_eq!(parse("ctrl+-").unwrap().canonical(), "ctrl+-");
        assert_eq!(parse("win+shift+s").unwrap().canonical(), "shift+win+s");
        assert_eq!(parse("alt+space").unwrap().canonical(), "alt+space");
        assert_eq!(display(parse("print").unwrap()), "PrtScn");
        assert_eq!(display(parse("pgdn").unwrap()), "PageDown");
    }

    /// The recorder writes a spec, the page shows `display`, and what the user
    /// reads back must still mean the same binding.
    #[test]
    fn display_round_trips_into_the_same_spec() {
        for text in [
            "ctrl+shift+a",
            "print",
            "f13",
            "ctrl++",
            "win+d",
            "alt+space",
            "ctrl+alt+delete",
            "shift+win+pageup",
            "ctrl+end",
        ] {
            let spec = parse(text).unwrap();
            let shown = display(spec);
            assert_eq!(parse(&shown).unwrap(), spec, "{text} -> {shown}");
        }
    }

    #[test]
    fn nonsense_is_refused_with_a_reason() {
        assert_eq!(parse("").unwrap_err(), HotkeyError::Empty);
        assert_eq!(parse("ctrl+").unwrap_err(), HotkeyError::Empty);
        assert_eq!(
            parse("hyper+a").unwrap_err(),
            HotkeyError::UnknownToken("hyper".into())
        );
        assert_eq!(parse("f25").unwrap_err(), HotkeyError::BadFunction(25));
        assert_eq!(
            parse("tabx").unwrap_err(),
            HotkeyError::UnknownToken("tabx".into())
        );
        // A bare letter would eat the user's typing everywhere in the session.
        assert_eq!(
            parse_bindable("a").unwrap_err(),
            HotkeyError::NeedsModifier("A".into())
        );
        assert_eq!(
            parse_bindable("f1").unwrap_err(),
            HotkeyError::NeedsModifier("F1".into())
        );
        assert!(parse_bindable("f13").is_ok());
        assert!(parse_bindable("print").is_ok());
    }

    #[test]
    fn conflicts_name_the_features_involved() {
        let mut map = BTreeMap::new();
        map.insert("capture".to_string(), "ctrl+shift+a".to_string());
        map.insert("solo".to_string(), "shift+CTRL+A".to_string());
        map.insert("paste_pin".to_string(), "ctrl+shift+v".to_string());
        let found = conflicts(&map);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].canonical, "ctrl+shift+a");
        assert_eq!(found[0].display, "Ctrl+Shift+A");
        assert_eq!(found[0].actions, ["capture", "solo"]);
        assert_eq!(found[0].reserved, None);

        // Two actions on a shell-owned combination is reported once, with the
        // reason that actually applies.
        let mut reserved = BTreeMap::new();
        reserved.insert("switch_group".to_string(), "win+d".to_string());
        let found = conflicts(&reserved);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].actions, ["switch_group"]);
        assert_eq!(found[0].reserved, Some("shows the desktop"), "{found:?}");
    }

    #[test]
    fn unbound_and_invalid_specs_are_not_conflicts() {
        let mut map = BTreeMap::new();
        map.insert("capture".to_string(), String::new());
        map.insert("solo".to_string(), String::new());
        map.insert("pin_edit".to_string(), "ctrl+shift+e".to_string());
        assert!(conflicts(&map).is_empty());
        let bad = invalid(&map);
        assert!(bad.is_empty(), "{bad:?}");

        let mut broken = BTreeMap::new();
        broken.insert("capture".to_string(), "nope".to_string());
        broken.insert("solo".to_string(), "win+l".to_string());
        let bad = invalid(&broken);
        assert_eq!(bad.len(), 2, "{bad:?}");
        assert_eq!(bad[0].0, "capture");
        assert!(bad[0].2.contains("nope"), "{bad:?}");
        assert_eq!(bad[1].0, "solo");
        assert!(bad[1].2.contains("lock"), "{bad:?}");
    }

    #[test]
    fn the_default_bindings_are_clean() {
        let cfg = crate::config::Config::default();
        assert!(invalid(&cfg.hotkey.actions).is_empty());
        assert!(conflicts(&cfg.hotkey.actions).is_empty());
        for spec in cfg.hotkey.actions.values() {
            assert!(parse(spec).is_ok(), "{spec}");
        }
    }

    #[test]
    fn exclusions_match_by_name_path_and_wildcard() {
        assert!(app_matches("game.exe", "C:\\Games\\GAME.exe"));
        assert!(app_matches("game.exe", "game.exe"));
        assert!(app_matches("*/teamviewer.exe", "c:/prog/teamviewer.exe"));
        assert!(app_matches("pres*", "C:/Apps/presentation.exe"));
        assert!(!app_matches("game.exe", "C:/Games/game2.exe"));
        assert!(!app_matches("", "game.exe"));
        assert!(!app_matches("game.exe", ""));

        let rules = vec![
            AppExclusion {
                app: "game.exe".into(),
                actions: vec!["capture".into()],
            },
            AppExclusion {
                app: "presentation.exe".into(),
                actions: vec![],
            },
        ];
        assert!(is_suppressed(&rules, "Game.exe", "capture"));
        assert!(!is_suppressed(&rules, "game.exe", "solo"));
        assert!(is_suppressed(&rules, "C:\\Deck\\presentation.exe", "solo"));
        assert!(!is_suppressed(&rules, "notepad.exe", "capture"));
    }

    #[test]
    fn every_default_action_is_a_known_id() {
        let cfg = crate::config::Config::default();
        for id in cfg.hotkey.actions.keys() {
            assert!(
                crate::config::ACTIONS.contains(&id.as_str()),
                "{id} is not a documented action"
            );
        }
    }
}
