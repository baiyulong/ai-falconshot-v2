//! The look this machine asked for (PRD §5.19.1 跟随系统主题).
//!
//! Windows keeps the answer in one registry value - `AppsUseLightTheme`, a DWORD under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`, where 1 means light
//! and 0 means dark. "Absent" is not a third look: it means the machine did not answer,
//! which is why this returns `Option<bool>` rather than `bool` - a `bool` would have to
//! guess, and the whole point of §5.19.1 is that the asking happens here.
//!
//! The decode is kept apart from the call so all its arms (no value / 0 / 1 / something
//! that is neither) are gradable in `cargo test`, while the live Win32 arm is graded by the
//! exe's own `[theme cfg]` line at startup. That split is the same one §5.4.3's cursor leg
//! uses: pure fork under test, platform read under a runtime witness.

/// `REG_DWORD`'s own number. Spelled here rather than imported from the `windows` crate so
/// [`decode_dword`] stays gradable on a machine that has no registry at all.
const REG_DWORD_KIND: u32 = 4;

/// The registry's bytes, read as what they claim to be: a little-endian `REG_DWORD` exactly
/// four bytes long.
///
/// Every other shape answers `None`, including a `REG_SZ` carrying `"1"` - a theme stored as
/// text is not a theme this program will invent an interpretation for, and a truncated read
/// is not a value either.
pub fn decode_dword(kind: u32, bytes: &[u8]) -> Option<u32> {
    if kind != REG_DWORD_KIND {
        return None;
    }
    let raw: [u8; 4] = bytes.try_into().ok()?;
    Some(u32::from_le_bytes(raw))
}

/// What the value means: `1` = light, `0` = dark, anything else = the machine is not saying.
///
/// A `2` does get written by some third-party theme engines, and the honest reading of that
/// is "no answer" rather than "dark, because it isn't 1".
pub const fn light_from_dword(raw: Option<u32>) -> Option<bool> {
    match raw {
        Some(1) => Some(true),
        Some(0) => Some(false),
        _ => None,
    }
}

/// `AppsUseLightTheme` as this machine answered it, or `None` when it did not.
///
/// P119：这一枚自此是**进程内一次**的缓存，而不是每次开一趟注册表。三处问它 ——
/// `[theme cfg]` 那一行、`Session.theme` 那枚 qproperty、和 `hint palette` 那一格的期待值 ——
/// 各读各的 HKCU 就是让同一趟运行里存在两个「机器的答案」，而那正是 #281 的形状：显示的那份
/// 与判的那份不是同一次读数。缓存的本体在 P119 是 [`std::sync::OnceLock`]，「至多读一次」由
/// 那一枚类型担保而不是由注释担保。
///
/// P120：§5.19.1 **第 3 步**落地，那一枚缓存自此有了它唯一的失效点 ——
/// [`apps_use_light_theme_invalidate`]，由 `WM_SETTINGCHANGE` 的 handler 叫。所以本体换成
/// [`std::sync::Mutex`] 套 `Option`：还是「至多一问、答了就存着」，而广播到了的那一问可以
/// 重新问。P119 那句「进程内一次也恰好是第 3 步不存在的形状」自此不再是这一格的读数，
/// 它留在台账里作为那一轮的证人（#281 的处置没变：一趟之内机器的答案仍然只有一枚，变的只是
/// 广播能让那一枚重新问一次）。
#[cfg(windows)]
pub fn apps_use_light_theme() -> Option<bool> {
    let mut held = answer_slot();
    match *held {
        Some(answer) => answer,
        None => {
            let answer = apps_use_light_theme_read();
            *held = Some(answer);
            answer
        }
    }
}

/// 让缓存重新问一次机器，并交出新的那一枚答案。§5.19.1 第 3 步的那一个字。
///
/// 只有 `WM_SETTINGCHANGE` 的 handler 该叫它。叫它的理由不是「时间过了很久」也不是「用户按了
/// 一次」，而是机器自己播了一次设置变更 —— 那一格由 [`setting_name`] 交出广播带来的名字。
#[cfg(windows)]
pub fn apps_use_light_theme_invalidate() -> Option<bool> {
    let answer = apps_use_light_theme_read();
    *answer_slot() = Some(answer);
    answer
}

/// 这一格的答案存在哪儿。`Option` 的 `None` 是「还没问过」而 `Some(None)` 是「问过、机器没答」：
/// 两臂不能合，否则一趟运行里会出现两次「机器的答案」而 #281 那一族又在界面上复活。
///
/// 中毒的锁照用：这一枚锁只保护一个 `Option<bool>`，一次 panic 不会让它处于半改状态，而
/// 「因为别处 panic 就此不再问机器」会让主题那一格静悄悄地停在旧值上。
#[cfg(windows)]
fn answer_slot() -> std::sync::MutexGuard<'static, Option<Option<bool>>> {
    static ANSWER: std::sync::Mutex<Option<Option<bool>>> = std::sync::Mutex::new(None);
    ANSWER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `WM_SETTINGCHANGE` 的 `lParam`：一段以 NUL 收尾的宽字符，写的是**哪一枚设置**变了
/// （主题那一臂是 `"ImmersiveColorSet"`），而当广播者是内核自己时那一格是 `NULL`。
///
/// 它是纯的，因为调用方交回的是一枚已经按上界读好的切片：本函数不碰指针，也不替「广播说什么」
/// 发明判据 —— 名字原样交出，是否主题由问机器那一格自己答。空切片与没有 NUL 的两臂都在这里
/// 收敛，因为泵线程读到的正是这两种形状（`NULL` ⇒ 空，忘了收尾 ⇒ 上界内的那一段）。
pub fn setting_name(wide: &[u16]) -> String {
    let end = wide
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

/// The registry itself, asked by [`apps_use_light_theme`] when the cache has no answer yet
/// and asked again by [`apps_use_light_theme_invalidate`] when the machine broadcast one.
#[cfg(windows)]
fn apps_use_light_theme_read() -> Option<bool> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, REG_VALUE_TYPE, RRF_RT_REG_DWORD,
    };

    let sub = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let val = wide("AppsUseLightTheme");
    let mut bytes = [0u8; 4];
    let mut cb = bytes.len() as u32;
    let mut kind = REG_VALUE_TYPE(0);
    // SAFETY: both `PCWSTR`s point into buffers that outlive this call and are
    // null-terminated; `bytes`, `cb` and `kind` are ours; and `RRF_RT_REG_DWORD` tells the
    // API to refuse anything that is not a DWORD rather than overflow the four-byte buffer.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sub.as_ptr()),
            PCWSTR(val.as_ptr()),
            RRF_RT_REG_DWORD,
            Some(&mut kind),
            Some(bytes.as_mut_ptr().cast()),
            Some(&mut cb),
        )
    };
    if status.is_err() {
        return None;
    }
    let read = (cb as usize).min(bytes.len());
    light_from_dword(decode_dword(kind.0, &bytes[..read]))
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The MVP is Windows-only, and a machine with no registry simply does not answer - the same
/// `None` the Windows arm returns when `AppsUseLightTheme` is absent. So the resolver's
/// "silent machine" arm is the one a non-Windows build actually takes, not a hypothetical one.
#[cfg(not(windows))]
pub fn apps_use_light_theme() -> Option<bool> {
    None
}

/// 没有注册表可问的机器也就没有可失效的缓存：这一格交回的和 [`apps_use_light_theme`] 是同一个
/// `None`，存在的理由只是让 §5.19.1 第 3 步那一臂的调用点不必再套一层 `#[cfg]`。
#[cfg(not(windows))]
pub fn apps_use_light_theme_invalidate() -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four arms, each of which the live path can actually reach: the type is not a DWORD,
    /// four bytes little-endian both ways, and a length that is not four.
    #[test]
    fn decode_takes_only_a_little_endian_dword() {
        assert_eq!(decode_dword(REG_DWORD_KIND, &[1, 0, 0, 0]), Some(1));
        assert_eq!(decode_dword(REG_DWORD_KIND, &[0, 0, 0, 0]), Some(0));
        assert_eq!(decode_dword(REG_DWORD_KIND, &[0, 0, 0, 1]), Some(1 << 24));
        // REG_SZ / REG_BINARY: a value that only looks like an answer.
        assert_eq!(decode_dword(1, &[1, 0, 0, 0]), None);
        assert_eq!(decode_dword(3, &[1, 0, 0, 0]), None);
        // Three bytes and five bytes are both "not a DWORD I can hand back".
        assert_eq!(decode_dword(REG_DWORD_KIND, &[1, 0, 0]), None);
        assert_eq!(decode_dword(REG_DWORD_KIND, &[1, 0, 0, 0, 0]), None);
        assert_eq!(decode_dword(REG_DWORD_KIND, &[]), None);
    }

    /// `2` is written by third-party theme engines, and reading it as "dark" would be a guess
    /// dressed up as a reading. `None` is the answer that can be re-checked.
    #[test]
    fn only_zero_and_one_answer_the_machine() {
        assert_eq!(light_from_dword(None), None);
        assert_eq!(light_from_dword(Some(0)), Some(false));
        assert_eq!(light_from_dword(Some(1)), Some(true));
        assert_eq!(light_from_dword(Some(2)), None);
        assert_eq!(light_from_dword(Some(u32::MAX)), None);
    }

    /// P120 §5.19.1 第 3 步：广播带来的那枚名字。四臂都是泵线程真会交回的形状 —— 带收尾符的、
    /// `NULL`（空切片）、忘了收尾符的、以及收尾符后面还跟着别人字节的。最后一臂是判据本体：
    /// 名字止于第一个 NUL，而不是止于缓冲区末尾。
    #[test]
    fn the_broadcast_name_stops_at_its_terminator() {
        let immersive: Vec<u16> = "ImmersiveColorSet".encode_utf16().chain([0]).collect();
        assert_eq!(setting_name(&immersive), "ImmersiveColorSet");
        assert_eq!(setting_name(&[]), "");
        let unterminated: Vec<u16> = "Theme".encode_utf16().collect();
        assert_eq!(setting_name(&unterminated), "Theme");
        let mut both: Vec<u16> = "Theme".encode_utf16().chain([0]).collect();
        both.extend("Colors".encode_utf16());
        assert_eq!(setting_name(&both), "Theme");
        // 一段不是名字的码元不猜：lossy 交出替换符，读到的那一格仍然被印出来而不是被丢弃。
        assert_eq!(setting_name(&[0xD800, 0]), "\u{FFFD}");
    }
}
