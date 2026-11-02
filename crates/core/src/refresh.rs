//! §5.5.8 刷新截图: the one decision a refresh makes, and the sentences it says.
//!
//! The requirement is four steps and two rules, and only one of the four steps is a
//! decision rather than a piece of work: 步骤 3「系统按原截图坐标重新读取屏幕内容」has to
//! answer *which* rectangle, and 步骤 4 plus 规则 1 have to answer *where the answer goes*.
//! Reading the screen and painting the result belong to the bridge, which owns the capture
//! service and the live set. What belongs here is the judgement, and it is worth its own
//! module for one reason: [`crate::imageops::still_valid`] has been the documented owner of
//! 规则 2「区域失效时应提示重新选择」since the rounded-corner round, and this is the flow
//! that finally asks it.

use crate::geometry::PhysRect;
use crate::imageops::still_valid;
use crate::pin::PinItem;

/// What §5.5.8 步骤 3/4 turns into, for one pin and one desktop.
///
/// The two success arms carry the rectangle to read again rather than a frame: this module
/// does not own a screen, and a decision that reached for pixels would be a second authority
/// over whether the desktop still holds the region — which [`still_valid`] already answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refresh {
    /// 规则「用户可选择覆盖原图或生成新图」的**覆盖原图**: read `0` again and put the new
    /// pixels under this pin, leaving the window where the user left it.
    Overwrite(PhysRect),
    /// The same rule's **生成新图**: read `0` again and show it as another pin, so the one
    /// on screen keeps the picture the user was looking at. 步骤 4 spells that choice
    /// 创建新副本, which is the same thing in a different register.
    NewCopy(PhysRect),
    /// Change nothing and hand the user a sentence.
    ///
    /// Two facts land here, and each says which of them it is: 规则 2 的那一枚失效区域, and
    /// the origin that never read the screen at all. They are one arm because the caller's
    /// conduct is identical — no re-read, no new pin, no moved pixel, one line of output —
    /// while the *words* must not be, and because a refusal that had two arms would have to
    /// be routed twice by code whose only question is "did anything change".
    Refused(String),
}

/// The whole of §5.5.8's 步骤 3 and both of its 规则, decided once.
///
/// `overwrite` is the user's choice from 规则 1, taken as an argument rather than read from
/// a setting: the PRD puts it on the action («用户可选择»), and the only stored choice in
/// this area is `[output] reuse_last_file`, which is §5.5.8's *file* half — the quick save
/// that overwrites the last landed file instead of incrementing a sequence. Letting that key
/// also decide a pin's pixels would be two requirements reading one slot.
///
/// The desktop is the union of the monitors, because the region may live on a secondary one
/// whose origin is negative (§5.3.10's cross-screen capture is what puts a pin there).
pub fn plan(pin: &PinItem, desktop: &PhysRect, overwrite: bool) -> Refresh {
    let Some(want) = pin.captured_rect() else {
        return Refresh::Refused(format!(
            "贴图 {} 不是从屏幕上截下来的，没有可重读的原坐标：请重新截一次。",
            pin.id
        ));
    };
    match still_valid(&want, desktop) {
        None => Refresh::Refused(format!(
            "贴图 {} 原来的那块区域 {:?} 已经不在任何屏幕上了：请重新选择。",
            pin.id, want
        )),
        // A region the changed desktop only partly covers cannot be re-read as *this*
        // picture: the bitmap would come back smaller than the one the pin is showing, and
        // 步骤 4 promises to 更新结果 rather than to shrink it. 规则 2's 提示 is the honest
        // answer, and it is deliberately not §5.2.5's answer — a repeat is allowed to clip
        // and say so, because it is making a new picture out of whatever is left; a refresh
        // owes the user the picture it cannot deliver.
        Some(area) if area != want => Refresh::Refused(format!(
            "贴图 {} 原来的那块区域 {:?} 现在只剩 {:?}：按原坐标读不回同一张图，请重新选择。",
            pin.id, want, area
        )),
        Some(area) => {
            if overwrite {
                Refresh::Overwrite(area)
            } else {
                Refresh::NewCopy(area)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Pin as PinConfig;
    use crate::frame::Frame;
    use crate::geometry::PhysPoint;

    fn desktop() -> PhysRect {
        PhysRect::new(0, 0, 1_920, 1_080)
    }

    /// A pin 80×40 whose pixels came off the desktop at `at` — the shape the bridge's
    /// `add_captured` produces, built here with no screen behind it.
    fn captured(at: PhysPoint) -> PinItem {
        let frame = Frame::filled(80, 40, [9, 9, 9, 255]).expect("frame");
        let mut p = PinItem::from_frame(7, &frame, at, &PinConfig::default()).expect("pin");
        p.captured_at = Some(at);
        p
    }

    /// 步骤 1 offers 「保留截图结果**或**将其贴到屏幕上」, but a pin can also arrive from a
    /// file or the clipboard. Those have no 原截图坐标 and the refusal has to be the answer:
    /// a refresh that silently fell back to the pin's current position would re-read
    /// whatever now sits under the window and call it 更新结果.
    #[test]
    fn a_pin_that_never_read_the_screen_says_so() {
        let frame = Frame::filled(80, 40, [9, 9, 9, 255]).expect("frame");
        let p = PinItem::from_frame(7, &frame, PhysPoint::new(100, 100), &PinConfig::default())
            .expect("pin");
        assert_eq!(p.captured_rect(), None, "没有哪块屏幕交过这张图");

        for overwrite in [true, false] {
            let got = plan(&p, &desktop(), overwrite);
            let Refresh::Refused(said) = &got else {
                panic!("这张图没读过屏幕，{overwrite} 那一支却答了 {got:?}");
            };
            assert!(said.contains("重新截一次"), "说的是: {said}");
            assert!(!said.contains("重新选择"), "区域没有失效: {said}");
        }
    }

    /// 步骤 3, plus 规则 1 read as the user's choice rather than the region's: one pin,
    /// one desktop, four ways to answer.
    #[test]
    fn a_region_still_whole_on_the_desktop_reads_again_where_it_was() {
        let at = PhysPoint::new(300, 200);
        let p = captured(at);
        let want = p.captured_rect().expect("rect");
        assert_eq!(want, PhysRect::new(300, 200, 80, 40));

        assert_eq!(plan(&p, &desktop(), true), Refresh::Overwrite(want));
        assert_eq!(plan(&p, &desktop(), false), Refresh::NewCopy(want));

        // …and the same pin on a layout that only reaches further left, which is what a
        // secondary monitor's negative origin looks like inside the union.
        let two = PhysRect::new(-1_280, 0, 3_200, 1_080);
        assert_eq!(plan(&p, &two, true), Refresh::Overwrite(want));
    }

    /// The arm the whole field exists for: a dragged pin is refreshed where its picture was
    /// **captured**, not where the window sits now. Deriving the rectangle from `pos_phys`
    /// would pass this test only while nobody ever moves anything.
    #[test]
    fn a_dragged_pin_still_reads_the_region_it_came_from() {
        let mut p = captured(PhysPoint::new(300, 200));
        p.pos_phys = PhysPoint::new(900, 700);
        p.zoom = 150;
        p.sync_size();
        assert_ne!(
            p.window_rect(),
            p.captured_rect().expect("rect"),
            "窗已经走开了"
        );

        assert_eq!(
            plan(&p, &desktop(), true),
            Refresh::Overwrite(PhysRect::new(300, 200, 80, 40)),
            "重读的是原截图坐标，不是窗此刻占的那一块"
        );
    }

    /// §5.9.9's crop narrows what the pin *shows*; §5.5.8 re-reads what the pin *came
    /// from*. The two have to stay two rectangles, or 刷新 would quietly start reading only
    /// the cropped corner and call the rest of the picture gone.
    #[test]
    fn a_cropped_pin_refreshes_the_region_it_was_cut_out_of() {
        let mut p = captured(PhysPoint::new(300, 200));
        p.crop(&PhysRect::new(0, 0, 40, 20)).expect("crop");
        assert_eq!(p.natural_size().w, 40, "窗上只剩四分之一");

        assert_eq!(
            plan(&p, &desktop(), false),
            Refresh::NewCopy(PhysRect::new(300, 200, 80, 40)),
            "裁剪不改原截图坐标"
        );
    }

    /// 规则 2: a region that has gone off every screen is told about, not guessed at.
    #[test]
    fn a_region_off_every_screen_asks_for_a_new_choice() {
        let p = captured(PhysPoint::new(2_500, 200));
        let got = plan(&p, &desktop(), true);
        let Refresh::Refused(said) = &got else {
            panic!("区域已经不在任何屏幕上，却答了 {got:?}");
        };
        assert!(said.contains("不在任何屏幕"), "说的是: {said}");
        assert!(said.contains("重新选择"), "规则 2 那句提示得在场: {said}");
    }

    /// The clipped arm. `still_valid` answers *something* here, so an implementation that
    /// only asked "is there an intersection" could have called it a success; the sentence
    /// has to name the region that was lost, because that is what the user re-selects
    /// against.
    #[test]
    fn a_region_the_desktop_only_partly_covers_is_refused_not_clipped() {
        let p = captured(PhysPoint::new(1_880, 200));
        let got = plan(&p, &desktop(), true);
        let Refresh::Refused(said) = &got else {
            panic!("读回来的图会比这张小，那一支不该是成功: {got:?}");
        };
        assert!(said.contains("只剩"), "说的是: {said}");
        assert!(
            said.contains("1880"),
            "失效的那一块得印出来，用户才知道要重选什么: {said}"
        );
    }

    /// #281's proof, in the direction that matters: this module must not have re-decided
    /// which regions are live. For every pair below the verdict is exactly the one
    /// [`still_valid`] gave — whole region ⇒ an arm carrying that region, partial or absent
    /// ⇒ a refusal.
    #[test]
    fn validity_is_still_valids_answer_and_nothing_else() {
        let cases = [
            (PhysPoint::new(300, 200), desktop(), true),
            (PhysPoint::new(1_880, 200), desktop(), false),
            (PhysPoint::new(2_500, 200), desktop(), false),
            (
                PhysPoint::new(-1_200, 100),
                PhysRect::new(-1_280, 0, 3_200, 1_080),
                true,
            ),
        ];
        for (at, desk, whole) in cases {
            let p = captured(at);
            let want = p.captured_rect().expect("rect");
            let live = still_valid(&want, &desk);
            let got = plan(&p, &desk, true);
            match (whole, live, &got) {
                (true, Some(area), Refresh::Overwrite(g)) => {
                    assert_eq!(*g, area, "plan 改了 still_valid 交回的那一块")
                }
                (false, small, Refresh::Refused(_)) => assert_ne!(
                    small,
                    Some(want),
                    "{desk:?} 里 {want:?} 明明是完整的，plan 却拒了"
                ),
                (_, _, other) => panic!(
                    "still_valid 答 {:?} 而 plan 答 {:?}（完整={whole}）",
                    still_valid(&want, &desk),
                    other
                ),
            }
        }
    }
}
