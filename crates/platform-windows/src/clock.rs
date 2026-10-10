//! The wall clock a file name is built from (PRD §5.5.3's 日期、时间).
//!
//! [`falcon_core::naming::LocalStamp`] takes its reading from outside on purpose: core
//! has no timezone database, and the only answer to "what is o'clock *here*" worth
//! putting in a name is the machine's own. Two numbers travel rather than one, because
//! the pair is what makes a name a local name - the epoch second the picture was taken
//! at, and how far east of UTC this machine thinks it is right now, DST included.

/// `(whole seconds since the Unix epoch, minutes east of UTC)`.
///
/// The offset is read per call rather than cached: a mask left open across a DST
/// boundary would otherwise name its next file by the old offset, and the call is a
/// handful of fields copied out of `kernel32`, not a lookup.
#[cfg(windows)]
pub fn wall_clock() -> (i64, i32) {
    use windows::Win32::System::Time::{
        GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
    };

    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let mut tzi = TIME_ZONE_INFORMATION::default();
    // `Bias` is minutes to *add* to local time to reach UTC, so the value a name wants
    // - minutes east - is its negative. The return says which of the two biases is
    // current (`2` = daylight, anything else = standard); `TIME_ZONE_ID_INVALID` means
    // the machine would not say, and the honest reading of that is offset 0 rather than
    // a guess: a UTC-named file is visibly odd, a file named by a stale cached offset is
    // odd in a direction nothing can re-check.
    let kind = unsafe { GetTimeZoneInformation(&mut tzi) };
    if kind == TIME_ZONE_ID_INVALID {
        return (epoch, 0);
    }
    let bias = tzi.Bias
        + if kind == 2 {
            tzi.DaylightBias
        } else {
            tzi.StandardBias
        };
    (epoch, -bias)
}

/// The same pair, with no timezone to ask: UTC is the only clock a non-Windows build
/// can name honestly, which is also what the `offset_min = 0` rows of core's tests use.
#[cfg(not(windows))]
pub fn wall_clock() -> (i64, i32) {
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    (epoch, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reading has to be a *now* reading and a whole-minute offset, because every
    /// consumer is a file name: an epoch drifting by hours or an offset carrying
    /// seconds would put a stamp on the disk that no user can reconcile with their
    /// own taskbar.
    #[test]
    fn the_clock_is_now_and_the_offset_is_a_whole_minute() {
        let (epoch, offset_min) = wall_clock();
        assert!(
            epoch > 1_700_000_000,
            "the epoch is not a current one: {epoch}"
        );
        assert_eq!(offset_min % 60, 0, "a sub-minute offset: {offset_min}");
        // Every inhabited zone the machine can be configured for, including the
        // half-hour and three-quarter-hour ones.
        assert!(
            offset_min.abs() <= 14 * 60,
            "an offset no timezone runs on: {offset_min}"
        );
    }
}
