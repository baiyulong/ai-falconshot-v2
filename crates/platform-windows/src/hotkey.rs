//! The two things the shell hands this process from outside its windows:
//! global hot keys (PRD §5.2.1, §5.16) and the tray icon (§5.1.1, §5.2.2).
//!
//! They share a module because they share a Win32 message-only window, and that
//! window is the only address either of them can be delivered to:
//! `RegisterHotKey` posts `WM_HOTKEY` to the window named at registration time,
//! and `Shell_NotifyIconW` posts its callback message to the window named in the
//! `NOTIFYICONDATAW`. A `TrackPopupMenu` is modal to the thread that calls it, so
//! the menu cannot be shown from a thread that pumps nothing.
//!
//! That thread is therefore a pump of its own - plan §2.1's "独占一条带消息泵
//! 线程". It is *not* Qt's thread: this window is not a `QWindow`, so Qt's event
//! loop never sees these messages, and running a second pump on the GUI thread
//! would starve it. Reaching the GUI thread from here is the product layer's job,
//! through `CxxQtThread::queue` (spike P3, `spike/hello-cxxqt/src/pump.rs:178`).
//!
//! One pump per process, because the delivery sink is a process global. The
//! alternative - threading a context pointer through `CreateWindowExW`'s
//! `lpparam` and out of `WM_CREATE`'s `CREATESTRUCTW` - buys multi-pump support
//! this product has no use for, and the rest of this workspace already keeps one
//! of each live thing in a `Mutex` global (`qt_bridge/src/mask.rs:1111`,
//! `capture.rs:42`).
//!
//! The key table is numbers, not `windows`' constants, so `cargo test` can
//! exercise the mapping without a desktop. What keeps the numbers honest is
//! [`mapping_still_matches_the_sdk`], which compares each one against the
//! `VIRTUAL_KEY` the SDK defines - a table checked against itself would prove
//! nothing.

use falcon_core::hotkeys::{Key, Spec, Special};
use std::sync::mpsc::Receiver;

/// Shell modifier bits, as `RegisterHotKey` counts them.
pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
/// Hold a hot key down and the shell repeats `WM_HOTKEY` until it is released.
/// For a screenshot tool each repeat means another frozen desktop, so every
/// binding registered here asks for repeat suppression - the one flag that is in
/// none of the user's text, and so must never be read back as a rebind.
pub const MOD_NOREPEAT: u32 = 0x4000;

pub const VK_BACK: u32 = 0x08;
pub const VK_TAB: u32 = 0x09;
pub const VK_RETURN: u32 = 0x0D;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_SPACE: u32 = 0x20;
pub const VK_PRIOR: u32 = 0x21;
pub const VK_NEXT: u32 = 0x22;
pub const VK_END: u32 = 0x23;
pub const VK_HOME: u32 = 0x24;
pub const VK_LEFT: u32 = 0x25;
pub const VK_UP: u32 = 0x26;
pub const VK_RIGHT: u32 = 0x27;
pub const VK_DOWN: u32 = 0x28;
pub const VK_SNAPSHOT: u32 = 0x2C;
pub const VK_INSERT: u32 = 0x2D;
pub const VK_DELETE: u32 = 0x2E;
pub const VK_PAUSE: u32 = 0x13;
pub const VK_CAPITAL: u32 = 0x14;
pub const VK_SCROLL: u32 = 0x91;
pub const VK_NUMLOCK: u32 = 0x90;
pub const VK_F1: u32 = 0x70;
pub const VK_OEM_1: u32 = 0xBA;
pub const VK_OEM_2: u32 = 0xBF;
pub const VK_OEM_3: u32 = 0xC0;
pub const VK_OEM_4: u32 = 0xDB;
pub const VK_OEM_5: u32 = 0xDC;
pub const VK_OEM_6: u32 = 0xDD;
pub const VK_OEM_7: u32 = 0xDE;
pub const VK_OEM_MINUS: u32 = 0xBD;
pub const VK_OEM_PERIOD: u32 = 0xBE;
pub const VK_OEM_PLUS: u32 = 0xBB;
pub const VK_OEM_COMMA: u32 = 0xBC;

/// A spec in the two numbers the shell takes. The action id lives beside it in
/// [`Request`], because the number `RegisterHotKey` echoes through `WM_HOTKEY` is
/// ours to choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub modifiers: u32,
    pub vk: u32,
}

impl Binding {
    /// Repeat suppression, added once here rather than at each call site that
    /// could forget it.
    pub fn with_norepeat(mut self) -> Self {
        self.modifiers |= MOD_NOREPEAT;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BindError {
    /// A key the grammar accepts but this table has no code for - a character
    /// outside ASCII, or `F25`.
    #[error("没有对应的虚拟键码：{0}")]
    NoVirtualKey(String),
    /// A combination Windows will not deliver globally (a bare `a`). Core already
    /// owns that rule (`Spec::is_bindable`, `hotkeys.rs:219`); this is only the
    /// report of it, so the same question cannot get two answers.
    #[error("Windows 不会把它全局投递：{0}")]
    Unbindable(String),
}

/// The virtual-key code for one key, or `None` when this table does not name it.
///
/// Letters and digits are arithmetic over their ASCII values because Win32's
/// `VK_A`…`VK_Z` and `VK_0`…`VK_9` are exactly that; the rest is a table. Punctuation
/// is named by its US-layout key, which is the layout `RegisterHotKey` matches
/// against a physical key, so `ctrl+shift+,` fires on the same keycap in any
/// layout - and a user on another layout who reads the label as a different
/// character still gets the binding that was recorded, not a silent refusal.
pub fn virtual_key(key: Key) -> Option<u32> {
    let code = match key {
        Key::Char(c) => match c {
            '0'..='9' => 0x30 + (c as u32 - '0' as u32),
            'a'..='z' => 0x41 + (c as u32 - 'a' as u32),
            '-' | '_' => VK_OEM_MINUS,
            '=' | '+' => VK_OEM_PLUS,
            '[' | '{' => VK_OEM_4,
            ']' | '}' => VK_OEM_6,
            '\\' | '|' => VK_OEM_5,
            ';' | ':' => VK_OEM_1,
            '\'' | '"' => VK_OEM_7,
            ',' | '<' => VK_OEM_COMMA,
            '.' | '>' => VK_OEM_PERIOD,
            '/' | '?' => VK_OEM_2,
            '`' | '~' => VK_OEM_3,
            _ => return None,
        },
        Key::Func(n) => {
            if (1..=24).contains(&n) {
                VK_F1 + u32::from(n - 1)
            } else {
                return None;
            }
        }
        Key::Special(s) => match s {
            Special::Escape => VK_ESCAPE,
            Special::Tab => VK_TAB,
            Special::Enter => VK_RETURN,
            Special::Space => VK_SPACE,
            Special::Backspace => VK_BACK,
            Special::Insert => VK_INSERT,
            Special::Delete => VK_DELETE,
            Special::Home => VK_HOME,
            Special::End => VK_END,
            Special::PageUp => VK_PRIOR,
            Special::PageDown => VK_NEXT,
            Special::Up => VK_UP,
            Special::Down => VK_DOWN,
            Special::Left => VK_LEFT,
            Special::Right => VK_RIGHT,
            Special::PrintScreen => VK_SNAPSHOT,
            Special::ScrollLock => VK_SCROLL,
            Special::NumLock => VK_NUMLOCK,
            Special::CapsLock => VK_CAPITAL,
            Special::Pause => VK_PAUSE,
        },
    };
    Some(code)
}

/// Turn a spec into the shell's two numbers, refusing what the shell would swallow
/// or never deliver. `canonical` is the text to name in an error, so a refusal
/// quotes the user's own binding back at them.
pub fn bind(spec: Spec, canonical: &str) -> Result<Binding, BindError> {
    if !spec.is_bindable() {
        return Err(BindError::Unbindable(canonical.to_string()));
    }
    let vk = virtual_key(spec.key).ok_or_else(|| BindError::NoVirtualKey(canonical.to_string()))?;
    let mut modifiers = 0u32;
    if spec.mods.ctrl {
        modifiers |= MOD_CONTROL;
    }
    if spec.mods.alt {
        modifiers |= MOD_ALT;
    }
    if spec.mods.shift {
        modifiers |= MOD_SHIFT;
    }
    if spec.mods.win {
        modifiers |= MOD_WIN;
    }
    Ok(Binding { modifiers, vk }.with_norepeat())
}

/// One action's worth of registration: the id this process chose, the feature it
/// belongs to, and what the user's text turned into. The action id travels with
/// the binding because a refusal has to name the feature, not the number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u32,
    pub action: String,
    pub canonical: String,
    pub binding: Binding,
}

/// What the pump thread decided the user did. Every variant is a fact about a
/// shell message; none of them knows what a screenshot tool should do about it,
/// which is what keeps this module testable without a product.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A registered combination arrived; `id` is a [`Request::id`].
    Hotkey(u32),
    /// A tray-menu command the user picked. `TrackPopupMenu` returned it, so this
    /// is a choice and not a click that landed somewhere.
    Menu(u32),
    /// A click on the icon itself. PRD §5.2.2 asks for "点击托盘图标" then a menu
    /// entry, and a left click that starts the one thing the tool is for satisfies
    /// both halves; the menu is on the right button.
    TrayLeft,
}

/// Why a registration did not happen, in the shell's own number plus a sentence a
/// user can act on. PRD §5.1.1 asks for a conflict report, and a report that
/// cannot name the reason is not one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub id: u32,
    pub action: String,
    pub canonical: String,
    /// `ERROR_HOTKEY_ALREADY_REGISTERED` and friends, read from `GetLastError`
    /// while it still describes this failure.
    pub win32_error: u32,
    pub why: &'static str,
}

/// The tray icon as the product describes it: a tooltip and a menu. The graphic is
/// the shell's own application icon, because this workspace has no brand `.ico`
/// yet - `packaging/` ships an exe and a QML tree, and PRD §5.1.1's "正确反映应用
/// 运行状态" is met here by the tooltip text, which [`Pump::set_tip`] rewrites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraySpec {
    pub tip: String,
    /// `(command id, label)`, in the order the menu shows them.
    pub items: Vec<(u32, String)>,
}

/// What the first registration pass learned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Started {
    /// `(id, action)` for every combination the shell accepted.
    pub registered: Vec<(u32, String)>,
    pub refused: Vec<Refused>,
    /// Whether the icon is actually in the notification area. A tray that refused
    /// is reported, not retried: the keys still work either way.
    pub tray: bool,
    pub hwnd: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    #[error("这个进程已经有一个热键泵了：一条线程一个窗口，不注册两次")]
    AlreadyRunning,
    #[error("建不出消息窗口：{0}")]
    Window(String),
    #[error("起不了消息泵线程：{0}")]
    Thread(String),
}

/// A running pump. Not dropping it without [`Pump::stop`] leaves the thread alive
/// on purpose: the thread owns the window the shell delivers to, and killing the
/// Rust handle would leave registered keys pointing at a window nobody pumps.
pub struct Pump {
    /// The message-only window the shell delivers to, kept as a number so this
    /// type has no Win32 dependency on a machine that has none.
    pub hwnd: u64,
    #[cfg(windows)]
    thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(windows)]
    events: Option<Receiver<Event>>,
}

#[cfg(windows)]
impl Pump {
    /// Build the window, register every request, install the tray icon if one was
    /// asked for, and report what the shell refused - all before this returns, so
    /// a caller can print the truth about its own hot keys.
    pub fn start(
        requests: Vec<Request>,
        tray: Option<TraySpec>,
    ) -> Result<(Pump, Started), StartError> {
        use std::sync::mpsc::channel;

        let (events_tx, events_rx) = channel::<Event>();
        let (handshake_tx, handshake_rx) = channel::<Started>();
        win::claim_sink(events_tx)?;
        win::stage(requests, tray);

        let spawned = std::thread::Builder::new()
            .name("falconshot-hotkey".into())
            .spawn(move || win::pump_thread(handshake_tx));
        let thread = match spawned {
            Ok(thread) => thread,
            Err(e) => {
                win::release_sink();
                return Err(StartError::Thread(e.to_string()));
            }
        };

        // The window, the registrations and the icon all happen on that thread, so
        // none of it is knowable until the handshake lands. Waiting for it is the
        // difference between reporting a registration and assuming one: a thread
        // that died before building a window has to say so, and `recv()` on a
        // dropped sender says it as an error rather than as a zero.
        let started = handshake_rx
            .recv()
            .map_err(|e| StartError::Window(format!("消息泵线程没交出注册结果：{e}")))?;
        if started.hwnd == 0 {
            win::release_sink();
            return Err(StartError::Window(
                "消息窗口建不起来，一个热键都没注册".to_string(),
            ));
        }
        Ok((
            Pump {
                hwnd: started.hwnd,
                thread: Some(thread),
                events: Some(events_rx),
            },
            started,
        ))
    }

    /// Hand the event stream to whoever will drive the product from it. Separate
    /// from `start` so a door that only proves registration can leave it alone.
    pub fn take_events(&mut self) -> Option<Receiver<Event>> {
        self.events.take()
    }

    /// Post the delivery the shell would post for a pressed hot key: `WM_HOTKEY`
    /// naming this id, to this pump's own window.
    ///
    /// What is real here and what is not: the registration above is the shell's own
    /// (`RegisterHotKey` answered OK or refused with a code), and the window
    /// procedure that receives this is the one that procedure the shell registered.
    /// Only the keystroke is missing - `press` stands in for the shell's post so a
    /// door can drive the dispatch chain without taking over the user's keyboard.
    /// It therefore cannot show that *this combination* would have fired: a key
    /// another program took stays taken, and `Registered` already said so.
    ///
    /// An id this pump did not register is refused rather than delivered: a door
    /// that pressed a key the shell rejected would otherwise read back a product
    /// reaction as proof the rejected registration worked.
    pub fn press(&self, id: u32) -> bool {
        if !win::is_registered(id) {
            return false;
        }
        win::post_wparam(
            self.hwnd,
            windows::Win32::UI::WindowsAndMessaging::WM_HOTKEY,
            id,
        )
    }

    /// Rewrite the tooltip - PRD §5.1.1's "托盘图标正确反映应用运行状态". Posted to
    /// the pump thread rather than calling the shell from this one, because every
    /// `Shell_NotifyIconW` in this process belongs to the thread that owns the
    /// window.
    pub fn set_tip(&self, text: &str) -> bool {
        win::set_pending_tip(text);
        win::post(self.hwnd, win::WM_APP_TIP)
    }

    /// Unregister, remove the icon, end the thread. That order is the one the
    /// shell wants: an icon whose window dies first stays in the notification area
    /// until Explorer reloads it, and a key registered to a live window keeps
    /// swallowing the user's keystrokes until something unregisters it.
    pub fn stop(&mut self) {
        // Asking the thread to close its own window is the only order that leaves
        // nothing behind: the keys go first, then the icon, then the window, then
        // the thread. A failed post means the window is already gone (Explorer
        // restarted, or the pump died of its own accord) and the join below is
        // still how we learn the thread has actually finished.
        if !win::post(self.hwnd, windows::Win32::UI::WindowsAndMessaging::WM_CLOSE) {
            tracing::warn!("hotkey pump: no window to close; waiting for the thread");
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        win::release_sink();
    }
}

#[cfg(not(windows))]
impl Pump {
    pub fn start(
        _requests: Vec<Request>,
        _tray: Option<TraySpec>,
    ) -> Result<(Pump, Started), StartError> {
        Err(StartError::Window(
            "全局热键与托盘只在 Windows 上存在".to_string(),
        ))
    }

    pub fn take_events(&mut self) -> Option<Receiver<Event>> {
        None
    }

    pub fn press(&self, _id: u32) -> bool {
        false
    }

    pub fn set_tip(&self, _text: &str) -> bool {
        false
    }

    pub fn stop(&mut self) {}
}

/// The Win32 half: statics, the window procedure, and the thread that owns both.
#[cfg(windows)]
mod win {
    use super::{Event, Refused, Request, Started, TraySpec};
    use std::sync::mpsc::Sender;
    use std::sync::Mutex;
    use windows::Win32::UI::Shell::NOTIFYICONDATAW;

    /// The one sink, claimed by `Pump::start` and read by the window procedure.
    static SINK: Mutex<Option<Sender<Event>>> = Mutex::new(None);
    /// What this pump registered, so teardown unregisters exactly these ids rather
    /// than guessing at a range.
    static REGISTERED: Mutex<Vec<(u32, String)>> = Mutex::new(Vec::new());
    /// The setup the caller staged, taken once by the thread that starts the pump.
    static STAGED: Mutex<Option<(Vec<Request>, Option<TraySpec>)>> = Mutex::new(None);
    static TRAY: Mutex<Option<Tray>> = Mutex::new(None);
    static PENDING_TIP: Mutex<Option<String>> = Mutex::new(None);

    /// What the notification area entry is, without any handle in it: `HWND` and
    /// `HICON` are raw pointers, so a `static` may not hold one, and the shell only
    /// needs this record to rebuild the `NOTIFYICONDATAW` it was given - every
    /// field it reads back is either a number this module chose or text it can re-ask
    /// the window for.
    pub struct Tray {
        pub hwnd: u64,
        pub uid: u32,
        pub shown: bool,
        pub items: Vec<(u32, String)>,
    }

    /// The user messages this module defines. `WM_APP` is the first number the
    /// shell promises a normal application.
    pub const WM_APP_TIP: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;
    const TRAY_CALLBACK: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

    /// This process has one icon, so it has one id. The shell keys an entry on the
    /// `(window, id)` pair, and every later call - modify, delete - has to name the
    /// same pair or it addresses an entry that was never added.
    const TRAY_UID: u32 = 1;

    /// `ERROR_HOTKEY_ALREADY_REGISTERED` - the one refusal a user can act on
    /// (rebind, or close whatever took the key).
    const ERROR_ALREADY_REGISTERED: u32 = 1409;
    /// `ERROR_INVALID_PARAMETER` - not deliverable at all.
    const ERROR_INVALID_PARAMETER: u32 = 87;

    fn why_of(code: u32) -> &'static str {
        match code {
            ERROR_ALREADY_REGISTERED => "别的程序已经占了这串键",
            ERROR_INVALID_PARAMETER => "Windows 不接受这个组合",
            _ => "注册失败",
        }
    }

    pub fn claim_sink(tx: Sender<Event>) -> Result<(), super::StartError> {
        let Ok(mut slot) = SINK.lock() else {
            return Err(super::StartError::AlreadyRunning);
        };
        if slot.is_some() {
            return Err(super::StartError::AlreadyRunning);
        }
        *slot = Some(tx);
        Ok(())
    }

    pub fn release_sink() {
        if let Ok(mut slot) = SINK.lock() {
            *slot = None;
        }
    }

    fn sink() -> Option<Sender<Event>> {
        SINK.lock().ok().and_then(|slot| slot.clone())
    }

    pub fn stage(requests: Vec<Request>, tray: Option<TraySpec>) {
        if let Ok(mut held) = STAGED.lock() {
            *held = Some((requests, tray));
        }
    }

    pub fn set_pending_tip(text: &str) {
        if let Ok(mut held) = PENDING_TIP.lock() {
            *held = Some(text.to_string());
        }
    }

    pub fn post(hwnd: u64, msg: u32) -> bool {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

        // SAFETY: a message carrying no pointers, posted to a window this module
        // created; the procedure reads whatever it needs out of this module's
        // statics rather than out of the message.
        unsafe { PostMessageW(Some(to_hwnd(hwnd)), msg, WPARAM(0), LPARAM(0)).is_ok() }
    }

    /// The same post with a number in `wparam`, which is the only form that can
    /// imitate a delivery: the shell puts the hot key's id there and nothing else.
    pub fn post_wparam(hwnd: u64, msg: u32, wparam: u32) -> bool {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

        // SAFETY: as above, and the value carried is a `u32` id rather than a
        // pointer, so there is nothing for the receiving thread to dereference.
        unsafe {
            PostMessageW(Some(to_hwnd(hwnd)), msg, WPARAM(wparam as usize), LPARAM(0)).is_ok()
        }
    }

    /// Whether this id is one the shell accepted, read out of the record `start`
    /// wrote. Teardown clears it, so after a pump is stopped nothing presses.
    pub fn is_registered(id: u32) -> bool {
        REGISTERED
            .lock()
            .map(|held| held.iter().any(|(at, _)| *at == id))
            .unwrap_or(false)
    }

    fn to_hwnd(hwnd: u64) -> windows::Win32::Foundation::HWND {
        windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void)
    }

    /// A `NOTIFYICONDATAW` that identifies one entry: size, window, id. Each call
    /// then sets only the `uFlags` bits it means to touch, because the shell reads
    /// whatever the flags claim is present - a tooltip update that left `NIF_ICON`
    /// set would be handing back an icon handle this function never had.
    fn envelope(hwnd: u64, uid: u32) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: to_hwnd(hwnd),
            uID: uid,
            ..Default::default()
        }
    }

    fn encode(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn write_wide(slot: &mut [u16], text: &str) {
        for (at, unit) in slot.iter_mut().zip(text.encode_utf16()) {
            *at = unit;
        }
        if let Some(last) = slot.last_mut() {
            *last = 0;
        }
    }

    /// The pump thread's whole life: window, registrations, icon, loop, teardown.
    pub fn pump_thread(handshake: Sender<Started>) {
        use windows::core::PCWSTR;
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::Input::KeyboardAndMouse::HOT_KEY_MODIFIERS;
        use windows::Win32::UI::Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DispatchMessageW, GetMessageW, LoadIconW, RegisterClassExW,
            TranslateMessage, UnregisterClassW, HWND_MESSAGE, IDI_APPLICATION, MSG,
            WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSEXW,
        };

        let staged = STAGED
            .lock()
            .ok()
            .and_then(|mut held| held.take())
            .unwrap_or_default();
        let wide = encode(SUPER_CLASS);

        // SAFETY: the documented message-only window recipe. The class is
        // registered against this module's own procedure, the window's only parent
        // is `HWND_MESSAGE`, buffers handed to the shell are written with their own
        // capacity, and every handle made here is destroyed on this thread before
        // it returns.
        unsafe {
            let Ok(module) = GetModuleHandleW(None) else {
                let _ = handshake.send(Started::default());
                return;
            };
            // `GetModuleHandleW` answers with a module handle; the class and the
            // window both take an instance handle. They are the same number and the
            // SDK says so with its own `From`, so the conversion is named once here
            // rather than at each of the three places that needs it.
            let instance: windows::Win32::Foundation::HINSTANCE = module.into();
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                lpszClassName: PCWSTR(wide.as_ptr()),
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 {
                let _ = handshake.send(Started::default());
                return;
            }
            let created = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(wide.as_ptr()),
                PCWSTR(encode("falconshot").as_ptr()),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance),
                None,
            );
            let Ok(hwnd) = created else {
                let _ = UnregisterClassW(PCWSTR(wide.as_ptr()), Some(instance));
                let _ = handshake.send(Started::default());
                return;
            };

            // Registration belongs to the window and the window to this thread, so
            // the ids the shell will echo are decided here - the caller has no way
            // to know this handle exists until the handshake says so.
            let mut registered = Vec::new();
            let mut refused = Vec::new();
            for request in &staged.0 {
                let binding = request.binding;
                let modifiers = HOT_KEY_MODIFIERS(request.binding.modifiers);
                if super::register(hwnd, request.id, modifiers, binding.vk).is_ok() {
                    registered.push((request.id, request.action.clone()));
                } else {
                    let code = windows::Win32::Foundation::GetLastError().0;
                    refused.push(Refused {
                        id: request.id,
                        action: request.action.clone(),
                        canonical: request.canonical.clone(),
                        win32_error: code,
                        why: why_of(code),
                    });
                }
            }
            if let Ok(mut held) = REGISTERED.lock() {
                *held = registered.clone();
            }

            let mut tray_shown = false;
            if let Some(spec) = staged.1 {
                match LoadIconW(None, IDI_APPLICATION) {
                    Ok(icon) => {
                        let mut nid = envelope(hwnd.0 as u64, TRAY_UID);
                        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
                        nid.uCallbackMessage = TRAY_CALLBACK;
                        nid.hIcon = icon;
                        write_wide(&mut nid.szTip, &spec.tip);
                        // Version 0 on purpose: the newer callbacks (`NIN_SELECT`)
                        // need `NIM_SETVERSION` and its union field, while the
                        // classic mouse messages arrive whatever the version, and
                        // the menu below reads exactly those.
                        let added = Shell_NotifyIconW(NIM_ADD, &nid).0 != 0;
                        if !added {
                            tracing::warn!("tray: the notification area refused the icon");
                        }
                        if let Ok(mut held) = TRAY.lock() {
                            *held = Some(Tray {
                                hwnd: hwnd.0 as u64,
                                uid: TRAY_UID,
                                shown: added,
                                items: spec.items,
                            });
                        }
                        tray_shown = added;
                    }
                    Err(e) => tracing::warn!("tray: no icon to show: {e}"),
                }
            }

            let _ = handshake.send(Started {
                registered,
                refused,
                tray: tray_shown,
                hwnd: hwnd.0 as u64,
            });

            let mut msg = MSG::default();
            loop {
                let got = GetMessageW(&mut msg, None, 0, 0);
                if got.0 <= 0 {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            // The loop ends only after teardown, but a thread that leaves by some
            // other route still has to hand the notification area back clean.
            teardown(hwnd);
            let _ = UnregisterClassW(PCWSTR(wide.as_ptr()), Some(instance));
        }
    }

    /// The window procedure answers three deliveries and nothing else.
    unsafe extern "system" fn wndproc(
        hwnd: windows::Win32::Foundation::HWND,
        msg: u32,
        wparam: windows::Win32::Foundation::WPARAM,
        lparam: windows::Win32::Foundation::LPARAM,
    ) -> windows::Win32::Foundation::LRESULT {
        use windows::Win32::Foundation::LRESULT;
        use windows::Win32::UI::WindowsAndMessaging::{
            DefWindowProcW, DestroyWindow, PostQuitMessage, WM_CLOSE, WM_DESTROY, WM_HOTKEY,
            WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP,
        };

        match msg {
            WM_HOTKEY => {
                if let Some(tx) = sink() {
                    let _ = tx.send(Event::Hotkey(wparam.0 as u32));
                }
                LRESULT(0)
            }
            m if m == TRAY_CALLBACK => {
                let clicked = (lparam.0 as u32) & 0xFFFF;
                match clicked {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                        if let Some(tx) = sink() {
                            let _ = tx.send(Event::TrayLeft);
                        }
                    }
                    WM_RBUTTONUP => show_menu(hwnd),
                    _ => {}
                }
                LRESULT(0)
            }
            m if m == WM_APP_TIP => {
                apply_tip(hwnd);
                LRESULT(0)
            }
            WM_CLOSE => {
                teardown(hwnd);
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                // A message-only window's `DefWindowProcW` does not post the quit
                // message that ends `GetMessageW`, so ending the thread is said
                // here rather than assumed: without this arm `Pump::stop` would
                // send the close, see the window go, and then block on `join`
                // forever. Teardown has already run - `WM_CLOSE` is the only route
                // to this message from inside this process.
                teardown(hwnd);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    /// Show the menu at the pointer. `TrackPopupMenu` blocks this thread until the
    /// user picks or dismisses, which is why it is called from the pump and never
    /// from the product: a menu shown on the GUI thread would freeze the mask.
    fn show_menu(hwnd: windows::Win32::Foundation::HWND) {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, SetForegroundWindow,
            TrackPopupMenu, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD,
        };

        let Some(items) = TRAY
            .lock()
            .ok()
            .and_then(|held| held.as_ref().map(|t| t.items.clone()))
        else {
            return;
        };
        if items.is_empty() {
            return;
        }
        // SAFETY: a menu built and destroyed here, strings kept alive by the
        // `encoded` vector that outlives the call, and a command id read out of the
        // same return value the shell was told to fill by `TPM_RETURNCMD`.
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let encoded: Vec<Vec<u16>> = items.iter().map(|(_, label)| encode(label)).collect();
            for ((id, _), unit) in items.iter().zip(&encoded) {
                if AppendMenuW(menu, MENU_STRING, *id as usize, PCWSTR(unit.as_ptr())).is_err() {
                    let _ = DestroyMenu(menu);
                    return;
                }
            }
            let mut at = POINT::default();
            if GetCursorPos(&mut at).is_err() {
                let _ = DestroyMenu(menu);
                return;
            }
            // MSDN's own requirement for a message-only owner: without this the
            // menu stays on screen after the click that should have dismissed it.
            let _ = SetForegroundWindow(hwnd);
            let flags = TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RETURNCMD;
            // `TPM_RETURNCMD` makes the return value the command, and `-1` the
            // failure that has no command. Reading `-1` as `0xFFFF_FFFF` would
            // invent a menu item that was never added.
            let picked = TrackPopupMenu(menu, flags, at.x, at.y, None, hwnd, None).0;
            let _ = DestroyMenu(menu);
            if picked > 0 {
                if let Some(tx) = sink() {
                    let _ = tx.send(Event::Menu(picked as u32));
                }
            }
        }
    }

    fn apply_tip(_hwnd: windows::Win32::Foundation::HWND) {
        use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIF_TIP, NIM_MODIFY};

        let Some(text) = PENDING_TIP.lock().ok().and_then(|mut held| held.take()) else {
            return;
        };
        let Ok(mut held) = TRAY.lock() else { return };
        let Some(tray) = held.as_mut() else { return };
        if !tray.shown {
            return;
        }
        let mut nid = envelope(tray.hwnd, tray.uid);
        nid.uFlags = NIF_TIP;
        write_wide(&mut nid.szTip, &text);
        // SAFETY: a structure that names this thread's own window and the id under
        // which `NIM_ADD` registered it, with only the tooltip bit claimed.
        let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
    }

    /// Hand everything back: the keys, then the icon. Idempotent, because a pump
    /// can be closed both by `WM_CLOSE` and by its own exit path.
    fn teardown(hwnd: windows::Win32::Foundation::HWND) {
        use windows::Win32::UI::Input::KeyboardAndMouse::UnregisterHotKey;
        use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIM_DELETE};

        let ids = REGISTERED
            .lock()
            .ok()
            .map(|held| held.clone())
            .unwrap_or_default();
        for (id, _) in ids {
            // SAFETY: an id this thread registered against this window. The shell
            // drops a window's keys when the window dies anyway; asking is what
            // makes "unregistered" a reading rather than an assumption.
            let _ = unsafe { UnregisterHotKey(Some(hwnd), id as i32) };
        }
        if let Ok(mut held) = REGISTERED.lock() {
            held.clear();
        }
        if let Ok(mut held) = TRAY.lock() {
            if let Some(tray) = held.as_mut() {
                if tray.shown {
                    let nid = envelope(tray.hwnd, tray.uid);
                    // SAFETY: the notification area entry identified by this exact
                    // handle/id pair, added by this thread.
                    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
                    tray.shown = false;
                }
            }
        }
    }

    const MENU_STRING: windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS =
        windows::Win32::UI::WindowsAndMessaging::MF_STRING;

    const SUPER_CLASS: &str = "falconshot.hotkey.pump";
}

/// `RegisterHotKey` behind a function so the call site reads as an attempt whose
/// outcome is reported, not as a block of FFI.
#[cfg(windows)]
fn register(
    hwnd: windows::Win32::Foundation::HWND,
    id: u32,
    modifiers: windows::Win32::UI::Input::KeyboardAndMouse::HOT_KEY_MODIFIERS,
    vk: u32,
) -> windows_core::Result<()> {
    // SAFETY: a window handle owned by the calling thread and an id chosen by this
    // module; the shell's own error is read next door in `GetLastError`.
    unsafe {
        windows::Win32::UI::Input::KeyboardAndMouse::RegisterHotKey(
            Some(hwnd),
            id as i32,
            modifiers,
            vk,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use falcon_core::hotkeys::parse;

    fn bound(text: &str) -> Result<Binding, BindError> {
        let spec = parse(text).expect(text);
        let canonical = spec.canonical();
        bind(spec, &canonical)
    }

    #[test]
    fn the_shape_the_shell_wants() {
        assert_eq!(
            bound("ctrl+shift+a").unwrap(),
            Binding {
                modifiers: MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
                vk: 0x41
            }
        );
        // `print` is the shipped default for `capture_active_window`, and it is the
        // only default with no modifier at all - so this is the one row that proves
        // `is_bindable` is what lets it through, not a special case here.
        assert_eq!(
            bound("print").unwrap(),
            Binding {
                modifiers: MOD_NOREPEAT,
                vk: VK_SNAPSHOT
            }
        );
        assert_eq!(
            bound("ctrl+shift+p").unwrap(),
            Binding {
                modifiers: MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
                vk: 0x50
            }
        );
    }

    #[test]
    fn a_bare_letter_is_refused_before_the_shell_is_asked() {
        assert_eq!(bound("a"), Err(BindError::Unbindable("a".to_string())));
        assert_eq!(bound("ctrl+a").unwrap().vk, 0x41);
        assert_eq!(bound("f13").unwrap().vk, VK_F1 + 12);
        assert_eq!(bound("f12"), Err(BindError::Unbindable("f12".to_string())));
        // `f25` never reaches this table over the wire: core's grammar stops it at
        // the door (`hotkeys.rs:261`), so the honest reading here is that the door
        // is closed *and* that the table would still refuse the same key if a
        // hand-built spec carried it past the parser.
        assert!(parse("f25").is_err());
        assert_eq!(virtual_key(Key::Func(25)), None);
    }

    #[test]
    fn the_window_key_is_a_bit_and_not_a_letter() {
        let binding = bound("win+shift+s").unwrap();
        assert_eq!(binding.modifiers, MOD_WIN | MOD_SHIFT | MOD_NOREPEAT);
        assert_eq!(binding.vk, 0x53);
    }

    #[test]
    fn punctuation_and_named_keys_have_codes() {
        for (text, want) in [
            ("ctrl+-", VK_OEM_MINUS),
            ("ctrl+=", VK_OEM_PLUS),
            ("ctrl++", VK_OEM_PLUS),
            ("ctrl+,", VK_OEM_COMMA),
            ("ctrl+.", VK_OEM_PERIOD),
            ("ctrl+/", VK_OEM_2),
            ("ctrl+[", VK_OEM_4),
            ("ctrl+]", VK_OEM_6),
            ("ctrl+\\", VK_OEM_5),
            ("ctrl+;", VK_OEM_1),
            ("ctrl+'", VK_OEM_7),
            ("ctrl+`", VK_OEM_3),
            ("ctrl+space", VK_SPACE),
            ("ctrl+esc", VK_ESCAPE),
            ("ctrl+enter", VK_RETURN),
            ("ctrl+up", VK_UP),
            ("ctrl+pgdn", VK_NEXT),
        ] {
            assert_eq!(bound(text).unwrap().vk, want, "{text}");
        }
    }

    #[test]
    fn a_key_this_table_does_not_name_is_said_not_guessed() {
        let mut parsed = parse("ctrl+shift+a").unwrap();
        parsed.key = Key::Char('あ');
        let canonical = parsed.canonical();
        assert_eq!(
            bind(parsed, &canonical),
            Err(BindError::NoVirtualKey("ctrl+shift+あ".to_string()))
        );
    }

    #[test]
    fn every_default_action_text_becomes_a_binding() {
        // The shipped config's nine actions, read from core's own default rather
        // than retyped here, so a default that stops mapping fails this row.
        let defaults = falcon_core::config::Config::default();
        for (id, text) in &defaults.hotkey.actions {
            let spec =
                parse(text).unwrap_or_else(|e| panic!("{id} = {text:?} does not parse: {e}"));
            let canonical = spec.canonical();
            bind(spec, &canonical)
                .unwrap_or_else(|e| panic!("{id} = {text:?} will not register: {e}"));
        }
        assert!(!defaults.hotkey.actions.is_empty());
    }

    /// The cross-check the module doc promises: these numbers are only trustworthy
    /// while they equal the SDK's, and the SDK is what `RegisterHotKey` compares
    /// against. A table checked against itself would prove nothing.
    #[cfg(windows)]
    #[test]
    fn mapping_still_matches_the_sdk() {
        use windows::Win32::UI::Input::KeyboardAndMouse as sdk;
        let table: [(u32, u32); 36] = [
            (VK_BACK, sdk::VK_BACK.0 as u32),
            (VK_TAB, sdk::VK_TAB.0 as u32),
            (VK_RETURN, sdk::VK_RETURN.0 as u32),
            (VK_ESCAPE, sdk::VK_ESCAPE.0 as u32),
            (VK_SPACE, sdk::VK_SPACE.0 as u32),
            (VK_PRIOR, sdk::VK_PRIOR.0 as u32),
            (VK_NEXT, sdk::VK_NEXT.0 as u32),
            (VK_END, sdk::VK_END.0 as u32),
            (VK_HOME, sdk::VK_HOME.0 as u32),
            (VK_LEFT, sdk::VK_LEFT.0 as u32),
            (VK_UP, sdk::VK_UP.0 as u32),
            (VK_RIGHT, sdk::VK_RIGHT.0 as u32),
            (VK_DOWN, sdk::VK_DOWN.0 as u32),
            (VK_SNAPSHOT, sdk::VK_SNAPSHOT.0 as u32),
            (VK_INSERT, sdk::VK_INSERT.0 as u32),
            (VK_DELETE, sdk::VK_DELETE.0 as u32),
            (VK_PAUSE, sdk::VK_PAUSE.0 as u32),
            (VK_CAPITAL, sdk::VK_CAPITAL.0 as u32),
            (VK_SCROLL, sdk::VK_SCROLL.0 as u32),
            (VK_NUMLOCK, sdk::VK_NUMLOCK.0 as u32),
            (VK_F1, sdk::VK_F1.0 as u32),
            (VK_OEM_1, sdk::VK_OEM_1.0 as u32),
            (VK_OEM_2, sdk::VK_OEM_2.0 as u32),
            (VK_OEM_3, sdk::VK_OEM_3.0 as u32),
            (VK_OEM_4, sdk::VK_OEM_4.0 as u32),
            (VK_OEM_5, sdk::VK_OEM_5.0 as u32),
            (VK_OEM_6, sdk::VK_OEM_6.0 as u32),
            (VK_OEM_7, sdk::VK_OEM_7.0 as u32),
            (VK_OEM_MINUS, sdk::VK_OEM_MINUS.0 as u32),
            (VK_OEM_PERIOD, sdk::VK_OEM_PERIOD.0 as u32),
            (VK_OEM_PLUS, sdk::VK_OEM_PLUS.0 as u32),
            (VK_OEM_COMMA, sdk::VK_OEM_COMMA.0 as u32),
            (MOD_ALT, sdk::MOD_ALT.0),
            (MOD_CONTROL, sdk::MOD_CONTROL.0),
            (MOD_SHIFT, sdk::MOD_SHIFT.0),
            (MOD_WIN, sdk::MOD_WIN.0),
        ];
        for (mine, theirs) in table {
            assert_eq!(mine, theirs, "a key code drifted from the SDK");
        }
        assert_eq!(
            MOD_NOREPEAT,
            sdk::MOD_NOREPEAT.0,
            "repeat suppression is added to every binding, so a wrong number here \
             would refuse all of them"
        );
    }
}
