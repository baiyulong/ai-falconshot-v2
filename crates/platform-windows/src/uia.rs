//! §5.3.3/§5.3.4 的那一只手：UIA（UI Automation）客户端的元素链。
//!
//! **本模块只问数，一道也不判定。** 三条上界与四道闸全部住在
//! [`falcon_core::capture`]：要不要往下问是 [`element_walk_wanted`]，一层兄弟里谁装着这一
//! 点是 [`element_under`]，问上来的原始一串怎么收成能选的链是 [`element_chain_of`]。这不是
//! 分层洁癖，而是 2026-11-04 那条口径点名的事 —— 「错的矩形会静默选中错的区域」，判错的
//! 那一半必须能在没有桌面、没有 COM 的地方被测（§9.1 第十条），而问数那一半只有真机器能答。
//!
//! 链是**往下钻**出来的，不是 `ElementFromPoint` 问出来的：遮罩开着的时候屏幕上每一点的
//! 最上窗都是本程序自己那一扇全屏窗，按点问永远问到工具自己。所以调用方递进来的是它已经
//! 认定的那一扇窗（[`falcon_core::capture::window_at_in`] 的答案），这一手从
//! `ElementFromHandle` 起、沿着控制视图一层层往下走。
//!
//! [`element_walk_wanted`]: falcon_core::capture::element_walk_wanted
//! [`element_under`]: falcon_core::capture::element_under
//! [`element_chain_of`]: falcon_core::capture::element_chain_of

use falcon_core::geometry::{PhysPoint, PhysRect};

/// §5.3.3 步 2-3 要的链：`(owner` 这扇窗里`)` 被 `p` 压着的元素边界，**内层在前**。
///
/// 交回的是**原始**一串（core 的闸在 [`falcon_core::capture::CaptureService::element_chain`]
/// 那一层才过），顺序就是钻下来的顺序翻个个儿：最后钻到的那一枚在最前。
///
/// 跟着一起交回的是「这一串是**问完**的、还是被某枚上界拦停的」
/// （[`falcon_core::capture::ElementChain::aborted`]）。这一层是唯一说得出这一格的：它知道自己
/// 是在「再没有更小的元素装着这一点」那一格停的，还是在深度/步数/墙钟那一格停的。遮罩那一头
/// 只在**问完的那一空**上才许停止再问同一扇窗，所以这一格不许省 —— 见
/// [`falcon_core::capture::ElementChain`] 头上记的那一次真机读数。
#[cfg(windows)]
pub fn chain(owner: u32, p: PhysPoint) -> falcon_core::capture::ElementChain {
    use falcon_core::capture::{element_under, element_walk_wanted, ElementChain};
    use std::time::Instant;

    let Some((automation, walker)) = client() else {
        return ElementChain::walked(Vec::new());
    };
    // 锚在调用方给的那扇窗上。问不出它的元素（窗已经没了、提供方不答、非 UIA 的窗）就是
    // 空链 —— §5.3.3 预期结果第二句要的退化，不是这一手的失败。
    //
    // 交回 `aborted = true`：这一趟连链上第一枚都没看过，「这一扇窗没有控件」这一句它没有
    // 资格说。真机读数 2026-11-05：同一枚把手、同一个点，本线程第一趟在这里花掉 73 毫秒
    // （提供方被第一次跨进程询问唤起来），紧接着的两趟各花 12 毫秒问完整串 —— 所以「问不出」
    // 既可能是这一扇窗真的不答，也可能是热身，而这一层分不开两者，就把它记成**没问出来**。
    let Ok(root) = (unsafe { automation.ElementFromHandle(crate::proc::to_hwnd(u64::from(owner))) })
    else {
        return ElementChain { rects: Vec::new(), aborted: true };
    };
    // 钟从**锚问到之后**起。[`falcon_core::capture::ELEMENT_CHAIN_BUDGET_MS`] 那 50 毫秒量的
    // 是「一次 hover 愿意为**钻一层层兄弟**花多久」，而客户端构造（`client()` 里过套间、加载
    // `UIAutomationCore.dll`）与第一次跨进程的 `ElementFromHandle`（唤起提供方）都是一线程/一
    // 进程一次的热身：把任何一笔记在它头上，本线程的第一趟都会在 depth 0 直接 break、交回一张
    // 空表 —— 那是上面那一格真踩到的形状（73 毫秒 > 50 毫秒，钻都没钻）。热身是一次的事，
    // 之后每一趟都有完整的 50 毫秒可花，那才是这一枚上界该量的东西。
    let started = Instant::now();
    let mut current = root;
    // 外层在前地攒，最后整个翻过来：钻的顺序天然是由外到内，而链要内层在前。
    let mut found: Vec<PhysRect> = Vec::new();
    let mut depth = 0usize;
    let mut steps = 0usize;
    let mut aborted = false;
    loop {
        if !element_walk_wanted(depth, steps, elapsed_ms(&started)) {
            aborted = true;
            break;
        }
        // 这一层的兄弟要**问完**才答得出哪一枚最小（顺序不保证，见 `element_under` 头上）。
        let mut kids: Vec<(windows::Win32::UI::Accessibility::IUIAutomationElement, PhysRect)> =
            Vec::new();
        let mut kid = unsafe { walker.GetFirstChildElement(&current) }.ok();
        while let Some(el) = kid {
            steps += 1;
            if let Some(r) = bounds(&el) {
                kids.push((el.clone(), r));
            }
            if !element_walk_wanted(depth, steps, elapsed_ms(&started)) {
                aborted = true;
                break;
            }
            kid = unsafe { walker.GetNextSiblingElement(&el) }.ok();
        }
        let rects: Vec<PhysRect> = kids.iter().map(|(_, r)| *r).collect();
        let Some(chosen) = element_under(&rects, p) else {
            break;
        };
        found.push(kids[chosen].1);
        current = kids[chosen].0.clone();
        depth += 1;
    }
    found.reverse();
    ElementChain { rects: found, aborted }
}

/// 非 Windows 的那一答：**没有这一只手**，交回空链。
///
/// 空表不是失败而是这一层的合法答案（[`falcon_core::capture::WindowExtras::element_chain`]
/// 头上写的是同一句话）：§5.3.3 的退化路径与 §5.3.4 的「在不支持的平台应自动退化、不应阻塞
/// 截图」都由它满足，而把模块整块 `#[cfg]` 掉就会让「这台机器问不出元素」变成没有声明。
#[cfg(not(windows))]
pub fn chain(_: u32, _: PhysPoint) -> falcon_core::capture::ElementChain {
    falcon_core::capture::ElementChain::walked(Vec::new())
}

/// 这一次询问已经花掉的毫秒。上界 [`falcon_core::capture::ELEMENT_CHAIN_BUDGET_MS`] 说的是
/// 墙钟而不是问数，所以只有**每一枚元素问完**再读一次才拦得住慢的提供方。
#[cfg(windows)]
fn elapsed_ms(started: &std::time::Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

/// 一枚元素的边界，物理屏幕像素。
///
/// 量不出、或量出来是空的（宽或高非正）都答 `None`：这一手交回的是**读数**，「空的要丢」
/// 那条判据在 [`falcon_core::capture::element_chain_of`]，两处各写一遍就会各改一次。
#[cfg(windows)]
fn bounds(element: &windows::Win32::UI::Accessibility::IUIAutomationElement) -> Option<PhysRect> {
    let r = unsafe { element.CurrentBoundingRectangle() }.ok()?;
    let w = r.right - r.left;
    let h = r.bottom - r.top;
    if w <= 0 || h <= 0 {
        return None;
    }
    Some(PhysRect::new(r.left, r.top, w as u32, h as u32))
}

/// 本线程那一套 UIA 客户端接口（`IUIAutomation` + 控制视图的 `IUIAutomationTreeWalker`）。
///
/// 为什么是 `thread_local` 而不是每次 hover 各起一实例：客户端的构造要过 COM 套间、要
/// 加载 `UIAutomationCore.dll`，而 hover 是鼠标每动一次问一句的热路径（同
/// [`falcon_core::capture::CaptureService::windows`] 头上缓存候选表那一族教训）。为什么
/// 不跨线程共用一份：UIA 的客户端接口是套间亲和的，本仓读桌面的动作分布在 GUI 线程与按键
/// 泵线程上（§5.19.1 记的就是这个分布），一份共用的接口在两个套间之间传就是未定义的事。
///
/// 交回的是**克隆**（一次 `AddRef`）而不是借出：`walk` 的循环里要回调进 COM，攥着
/// `RefCell` 的借用去调提供方的代码，等于把本模块的锁交给别人死锁。
#[cfg(windows)]
fn client() -> Option<(
    windows::Win32::UI::Accessibility::IUIAutomation,
    windows::Win32::UI::Accessibility::IUIAutomationTreeWalker,
)> {
    use std::cell::RefCell;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};

    struct Cached {
        automation: IUIAutomation,
        walker: windows::Win32::UI::Accessibility::IUIAutomationTreeWalker,
    }

    thread_local! {
        static CLIENT: RefCell<Option<Cached>> = const { RefCell::new(None) };
    }

    CLIENT.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            // 与 [`crate::pick::pick`]、[`crate::reveal::select`] 逐字同一条理由：这一条线程
            // 可能早就被 Qt 起过 COM（剪贴板那一路就要），那时返回 `S_FALSE`；换个套间模型
            // 返回 `RPC_E_CHANGED_MODE`。两种都不必中止，也不在这里 `CoUninitialize` ——
            // 返回值是套间状态而不是成败，而这一套接口留到线程结束为止。
            let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            // `CUIAutomation` 在这里是那个 **clsid 常量**（`windows` 把 coclass 的 GUID 发成
            // 了同名的一枚常量），`IUIAutomation` 才是要拿的那个接口。
            let automation: IUIAutomation =
                unsafe { CoCreateInstance(&CUIAutomation, None::<&windows::core::IUnknown>, CLSCTX_INPROC_SERVER) }
                    .ok()?;
            let walker = unsafe { automation.ControlViewWalker() }.ok()?;
            *slot = Some(Cached { automation, walker });
        }
        slot.as_ref()
            .map(|c| (c.automation.clone(), c.walker.clone()))
    })
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// 一条真的 UIA 询问，对着**本进程自己**的一扇窗：真机器、真 COM、真提供方，所以它挂在
    /// `#[ignore]` 上（与 [`crate::win`] 那枚问 DWM 的格子同一规矩）—— 默认那一趟要的是
    /// 不碰桌面的判定，而这一格要的是一次真读数，两者不该互相冒充。
    ///
    /// 这一格**只**能说三件事，写在断言边上而不是这里以下：链要么空、要么每一枚都非空且
    /// 一枚比一枚小（钻下去是按着「最小的那一枚装着这一点」走的）。它**证不到**「屏幕上那个
    /// 按钮被认出来了」—— 那一句要的是把矩形摆回那张画面上看，只有眼睛能判。
    #[test]
    #[ignore]
    fn asks_uia_for_the_chain_inside_a_real_window() {
        use falcon_core::capture::CaptureService;
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow};

        let hwnd = unsafe { GetForegroundWindow() };
        assert!(!hwnd.0.is_null(), "这一台此刻没有前台窗口");
        let mut pt = POINT::default();
        let _ = unsafe { GetCursorPos(&mut pt) };
        let p = PhysPoint::new(pt.x, pt.y);
        // 同一枚把手、同一个点连问三趟。**这一格的形状就是 2026-11-05 踩到的那一格**：本线程
        // 的第一趟交回空表，第二、第三趟各交回三枚 —— 差的是提供方热身那几百毫秒。空表本身
        // 不是错（遮罩据此退回整窗检测），把空表说成「问完了、这一扇窗没有控件」才是错：
        // 遮罩只在「问完了」那一格上才许停止再问，于是第一趟的失明会一路跟着这一场。
        let first = chain(hwnd.0 as u32, p);
        let second = chain(hwnd.0 as u32, p);
        let third = chain(hwnd.0 as u32, p);
        println!("hwnd = {:?} point = {:?}", hwnd, p);
        println!("first = {first:?}");
        println!("second = {second:?}");
        println!("third = {third:?}");
        for r in &first.rects {
            assert!(!r.is_empty(), "原始链里不许有零大小的矩形");
        }
        assert_eq!(
            second, third,
            "同一枚把手同一个点，问完了的两趟不该各说一遍 —— 提供方稳住了才有这一格"
        );
        assert!(
            !first.rects.is_empty() || first.aborted,
            "第一趟交回空表却不说是被拦停的 = 谎报「这一扇窗没有控件」"
        );
        // 过闸那一条腿也要走一遍：服务层交回的是闸之后的链，不是这一手的原始读数。
        let svc = CaptureService::with_extras(
            Box::new(falcon_core::capture::XcapCapture),
            Box::new(crate::win::WinExtras),
        );
        let gated = svc.element_chain(hwnd.0 as u32, p);
        println!("gated = {gated:?}");
        assert!(
            gated.rects.len() <= third.rects.len(),
            "闸只会丢、只会裁，不会长：{} > {}",
            gated.rects.len(),
            third.rects.len()
        );
    }
}
