//! Hot corners (PRD §5.15): the pointer sits in a screen corner for
//! `dwell_ms`, and the action bound to that corner runs.
//!
//! The detection is deliberately boring arithmetic over the four vertices of
//! the virtual desktop, because the alternative — asking Windows — has no API.
//! The caller feeds pointer samples plus a monotonic clock in milliseconds, so
//! the dwell and the repeat guard are testable without sleeping.

use crate::capture::MonitorInfo;
use crate::config::{HotCorner, HOT_CORNER_ACTIONS};
use crate::geometry::{PhysPoint, PhysRect, Scale};
use crate::hotkeys::Modifiers;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub const ALL: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomLeft,
        Corner::BottomRight,
    ];

    /// The key this corner uses inside `[hot_corner.corners]`.
    pub fn key(self) -> &'static str {
        match self {
            Corner::TopLeft => "top_left",
            Corner::TopRight => "top_right",
            Corner::BottomLeft => "bottom_left",
            Corner::BottomRight => "bottom_right",
        }
    }

    pub fn from_key(key: &str) -> Option<Corner> {
        Corner::ALL.into_iter().find(|c| c.key() == key)
    }

    /// The vertex this corner is measured from, given the desktop bounds.
    fn vertex(self, desktop: &PhysRect) -> PhysPoint {
        PhysPoint::new(
            match self {
                Corner::TopLeft | Corner::BottomLeft => desktop.x,
                Corner::TopRight | Corner::BottomRight => desktop.right(),
            },
            match self {
                Corner::TopLeft | Corner::TopRight => desktop.y,
                Corner::BottomLeft | Corner::BottomRight => desktop.bottom(),
            },
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Firing {
    pub corner: Corner,
    pub action: String,
}

#[derive(Clone, Copy, Debug, Default)]
struct State {
    /// When the pointer first entered, or `None` when it is outside.
    entered_ms: Option<u64>,
    /// Already fired during this visit, so dwelling does not repeat.
    fired: bool,
    last_fired_ms: Option<u64>,
}

pub struct HotCorners {
    enabled: bool,
    dwell_ms: u64,
    guard_ms: u64,
    require: Modifiers,
    /// `None` = this corner is switched off.
    actions: [(Corner, Option<String>); 4],
    rects: [(Corner, PhysRect); 4],
    state: [State; 4],
    /// §5.15 规则: a fullscreen app or game owns the screen, so the corners wait.
    paused: bool,
}

/// The scale of the display a vertex belongs to. A vertex can sit exactly on
/// the exclusive right/bottom edge of the desktop, which no monitor's
/// half-open bounds contain, so probe a few pixels inside before giving up.
fn scale_of(monitors: &[MonitorInfo], p: PhysPoint) -> Scale {
    for (dx, dy) in [(0, 0), (-1, 0), (0, -1), (-1, -1)] {
        let probe = PhysPoint::new(p.x.saturating_add(dx), p.y.saturating_add(dy));
        if let Some(m) = monitors.iter().find(|m| m.bounds.contains(probe)) {
            return m.scale;
        }
    }
    monitors
        .iter()
        .find(|m| m.primary)
        .or_else(|| monitors.first())
        .map(|m| m.scale)
        .unwrap_or(Scale::of(1.0))
}

impl HotCorners {
    /// Build the trigger squares from the current config and display layout.
    /// The radius is a DIP measurement, so it is converted with the scale of
    /// the monitor that owns that vertex (PRD §4.3: no corner may be half the
    /// size the user asked for because a second screen is at 150%).
    pub fn new(cfg: &HotCorner, monitors: &[MonitorInfo]) -> Self {
        let desktop = if monitors.is_empty() {
            PhysRect::new(0, 0, 0, 0)
        } else {
            monitors
                .iter()
                .skip(1)
                .fold(monitors[0].bounds, |acc, m| acc.union(&m.bounds))
        };
        let radius_dip = cfg.trigger_dip.max(1);
        let actions = Corner::ALL.map(|c| {
            let bound = cfg
                .corners
                .get(c.key())
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty() && HOT_CORNER_ACTIONS.contains(&a.as_str()));
            (c, bound)
        });
        let rects = Corner::ALL.map(|c| {
            let vertex = c.vertex(&desktop);
            let scale = scale_of(monitors, vertex);
            let radius = if desktop.is_empty() {
                0
            } else {
                scale.dip_to_phys_i(radius_dip as i32).max(1) as u32
            };
            let rect = PhysRect::new(
                match c {
                    Corner::TopLeft | Corner::BottomLeft => vertex.x,
                    Corner::TopRight | Corner::BottomRight => {
                        vertex.x.saturating_sub(radius as i32)
                    }
                },
                match c {
                    Corner::TopLeft | Corner::TopRight => vertex.y,
                    Corner::BottomLeft | Corner::BottomRight => {
                        vertex.y.saturating_sub(radius as i32)
                    }
                },
                radius,
                radius,
            );
            (c, rect)
        });
        HotCorners {
            enabled: cfg.enabled,
            dwell_ms: cfg.dwell_ms as u64,
            guard_ms: cfg.repeat_guard_ms as u64,
            require: Modifiers {
                ctrl: cfg.require_ctrl,
                alt: false,
                shift: cfg.require_shift,
                win: false,
            },
            actions,
            rects,
            state: [State::default(); 4],
            paused: false,
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        if paused != self.paused {
            self.paused = paused;
            self.reset();
        }
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The configured radius of one corner, for the settings page to preview.
    pub fn rect(&self, corner: Corner) -> PhysRect {
        self.rects
            .iter()
            .find(|(c, _)| *c == corner)
            .map(|(_, r)| *r)
            .unwrap_or_default()
    }

    /// The corner under the pointer, if any. Two corners share a square only
    /// when the desktop is smaller than the radius, which the clamp below
    /// resolves in favour of the first listed corner.
    pub fn corner_at(&self, p: PhysPoint) -> Option<Corner> {
        self.rects
            .iter()
            .find(|(_, r)| r.contains(p))
            .map(|(c, _)| *c)
    }

    /// Feed one pointer sample. `Some` means run that action now.
    pub fn pointer(&mut self, p: PhysPoint, mods: Modifiers, now_ms: u64) -> Option<Firing> {
        if !self.enabled || self.paused {
            self.reset();
            return None;
        }
        let mods_ok = (!self.require.ctrl || mods.ctrl) && (!self.require.shift || mods.shift);
        let mut fired = None;
        for (slot, (corner, rect)) in self.rects.into_iter().enumerate() {
            let inside = mods_ok && rect.contains(p);
            let state = &mut self.state[slot];
            if !inside {
                state.entered_ms = None;
                state.fired = false;
                continue;
            }
            let entered = *state.entered_ms.get_or_insert(now_ms);
            if state.fired {
                continue;
            }
            if now_ms.saturating_sub(entered) < self.dwell_ms {
                continue;
            }
            state.fired = true;
            if let Some(last) = state.last_fired_ms {
                if now_ms.saturating_sub(last) < self.guard_ms {
                    continue;
                }
            }
            state.last_fired_ms = Some(now_ms);
            if let Some((_, Some(action))) = self.actions.get(slot) {
                fired = Some(Firing {
                    corner,
                    action: action.clone(),
                });
            }
        }
        fired
    }

    /// Forget every visit. The platform layer calls this when the pointer
    /// leaves the desktop or a capture mask appears, so a screenshot taken
    /// from a corner does not immediately trigger the corner again.
    pub fn reset(&mut self) {
        self.state = [State::default(); 4];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::MonitorInfo;
    use crate::config::Config;
    use crate::geometry::Scale;

    fn monitor(x: i32, y: i32, w: u32, h: u32, scale: f64, primary: bool) -> MonitorInfo {
        MonitorInfo {
            id: format!("{x},{y}"),
            name: "Test".into(),
            friendly: "Test".into(),
            bounds: PhysRect::new(x, y, w, h),
            scale: Scale::of(scale),
            primary,
            builtin: false,
        }
    }

    fn cfg(action: &str) -> HotCorner {
        let mut c = Config::default().hot_corner;
        c.enabled = true;
        c.corners.insert("bottom_right".into(), action.into());
        c
    }

    #[test]
    fn a_corner_must_be_dwelled_in_not_just_crossed() {
        let monitors = vec![monitor(0, 0, 1920, 1080, 1.0, true)];
        let mut hc = HotCorners::new(&cfg("capture"), &monitors);
        let point = PhysPoint::new(1919, 1079);
        // A sweep past the corner: 100 ms at the vertex is under the 300 ms dwell.
        assert_eq!(hc.pointer(point, Modifiers::default(), 0), None);
        assert_eq!(hc.pointer(point, Modifiers::default(), 100), None);
        let hit = hc.pointer(point, Modifiers::default(), 300);
        assert_eq!(
            hit,
            Some(Firing {
                corner: Corner::BottomRight,
                action: "capture".into()
            })
        );
        // Dwelling longer does not fire twice within one visit.
        assert_eq!(hc.pointer(point, Modifiers::default(), 900), None);
        // Leaving and coming back is a new visit, but the guard still applies.
        assert_eq!(
            hc.pointer(PhysPoint::new(800, 500), Modifiers::default(), 1000),
            None
        );
        assert_eq!(hc.pointer(point, Modifiers::default(), 1100), None);
        assert!(hc.pointer(point, Modifiers::default(), 2000).is_some());
    }

    #[test]
    fn only_the_corner_is_a_trigger_square() {
        let monitors = vec![monitor(0, 0, 1920, 1080, 1.0, true)];
        let mut hc = HotCorners::new(&cfg("capture"), &monitors);
        assert_eq!(
            hc.corner_at(PhysPoint::new(1919, 1079)),
            Some(Corner::BottomRight)
        );
        // Geometry knows all four corners; the action map decides whether an
        // unbound one does anything.
        assert_eq!(hc.corner_at(PhysPoint::new(0, 0)), Some(Corner::TopLeft));
        // The square is 8 px wide, so 1912 is its first column and 1911 is out.
        assert_eq!(
            hc.corner_at(PhysPoint::new(1912, 1079)),
            Some(Corner::BottomRight)
        );
        assert_eq!(hc.corner_at(PhysPoint::new(1911, 1079)), None);
        // The square sits inside the desktop, so the last pixel row counts.
        let r = hc.rect(Corner::BottomRight);
        assert_eq!(r.right(), 1920);
        assert_eq!(r.bottom(), 1080);
        assert_eq!(r.w, 8);
        // A pointer that only passes 200 px from the vertex never fires.
        assert_eq!(
            hc.pointer(PhysPoint::new(1700, 1000), Modifiers::default(), 0),
            None
        );
        assert_eq!(
            hc.pointer(PhysPoint::new(1700, 1000), Modifiers::default(), 5000),
            None
        );
    }

    #[test]
    fn the_radius_is_measured_in_dips_per_monitor() {
        // Second screen at 150%: its corner must be 12 physical pixels wide,
        // the primary's stays 8, and the union's top-right belongs to the second.
        let monitors = vec![
            monitor(0, 0, 1920, 1080, 1.0, true),
            monitor(1920, 0, 2560, 1440, 1.5, false),
        ];
        let mut hc = HotCorners::new(&cfg("capture"), &monitors);
        assert_eq!(hc.rect(Corner::TopLeft).w, 8);
        assert_eq!(hc.rect(Corner::TopRight).w, 12);
        assert_eq!(hc.rect(Corner::BottomRight).x, 4480 - 12);
        // 4479 is the last column of the 150% screen.
        assert_eq!(
            hc.corner_at(PhysPoint::new(4479, 1439)),
            Some(Corner::BottomRight)
        );
        // A dwell at 12 px of travel still counts as inside the corner.
        assert_eq!(
            hc.pointer(PhysPoint::new(4470, 1439), Modifiers::default(), 0),
            None
        );
        assert!(hc
            .pointer(PhysPoint::new(4470, 1439), Modifiers::default(), 400)
            .is_some());
    }

    #[test]
    fn disabled_or_unbound_corners_do_nothing() {
        let monitors = vec![monitor(0, 0, 1000, 800, 1.0, true)];
        let off = Config::default().hot_corner;
        let mut hc = HotCorners::new(&off, &monitors);
        assert!(!hc.enabled());
        assert_eq!(
            hc.pointer(PhysPoint::new(999, 799), Modifiers::default(), 10_000),
            None
        );

        let mut empty = cfg("fly");
        empty.corners.insert("bottom_right".into(), String::new());
        let mut hc = HotCorners::new(&empty, &monitors);
        assert_eq!(
            hc.pointer(PhysPoint::new(999, 799), Modifiers::default(), 10_000),
            None
        );
        assert_eq!(
            hc.pointer(PhysPoint::new(999, 799), Modifiers::default(), 20_000),
            None
        );

        // A corner bound to an action outside the five §5.15 entries is ignored.
        let mut wide = cfg("capture");
        wide.corners
            .insert("top_left".into(), "capture_repeat".into());
        let mut hc = HotCorners::new(&wide, &monitors);
        assert_eq!(
            hc.pointer(PhysPoint::new(0, 0), Modifiers::default(), 10_000),
            None
        );
    }

    #[test]
    fn a_required_modifier_arms_the_corner() {
        let monitors = vec![monitor(0, 0, 1000, 800, 1.0, true)];
        let mut c = cfg("capture");
        c.require_ctrl = true;
        let mut hc = HotCorners::new(&c, &monitors);
        let point = PhysPoint::new(999, 799);
        let none = Modifiers::default();
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        // Without Ctrl the corner is not entered at all, so releasing Ctrl in
        // the corner does not retroactively start the dwell.
        assert_eq!(hc.pointer(point, none, 0), None);
        assert_eq!(hc.pointer(point, none, 5_000), None);
        assert_eq!(hc.pointer(point, ctrl, 5_100), None);
        assert!(hc.pointer(point, ctrl, 5_500).is_some());
    }

    #[test]
    fn pause_is_for_fullscreen_apps_and_games() {
        let monitors = vec![monitor(0, 0, 1000, 800, 1.0, true)];
        let mut hc = HotCorners::new(&cfg("capture"), &monitors);
        let point = PhysPoint::new(999, 799);
        hc.set_paused(true);
        assert!(hc.paused());
        assert_eq!(hc.pointer(point, Modifiers::default(), 0), None);
        assert_eq!(hc.pointer(point, Modifiers::default(), 50_000), None);
        hc.set_paused(false);
        // The dwell restarts, because the pause cleared the visit.
        assert_eq!(hc.pointer(point, Modifiers::default(), 50_100), None);
        assert!(hc.pointer(point, Modifiers::default(), 50_500).is_some());
    }

    #[test]
    fn corner_keys_are_the_config_keys() {
        for c in Corner::ALL {
            assert_eq!(Corner::from_key(c.key()), Some(c));
        }
        assert_eq!(Corner::from_key("middle"), None);
    }

    #[test]
    fn no_monitors_is_not_a_panic() {
        let mut hc = HotCorners::new(&cfg("capture"), &[]);
        assert_eq!(hc.rect(Corner::TopLeft), PhysRect::default());
        assert_eq!(
            hc.pointer(PhysPoint::new(0, 0), Modifiers::default(), 9_000),
            None
        );
    }
}
