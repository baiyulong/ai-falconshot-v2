//! §5.6.1 的第 3 步：把一张 [`Frame`] 交给系统自带的识别引擎，取回文字。
//!
//! 选 `Windows.Media.Ocr` 而不是任何在线服务，同时回答 PRD 的两条：§5.6.1「若识别
//! 依赖在线服务，应明确显示网络和隐私提示」在这里的答案是**图不出机器**（引擎随系统
//! 语言包装在本机），而 §7.5 `privacy_no_temp_files` 要的"不落盘"则要求像素从头到尾
//! 待在内存里 —— 所以这里是一张 `Vec<u8>` 换一枚内存 `IBuffer`，没有临时文件，也没有
//! 一次 PNG 编解码的往返。
//!
//! 换行是自己拼的，没有取 `OcrResult::Text()`：那一格在中文引擎上把各行直接连起来，
//! 换行消失而段落实心化，正好违掉 §5.6.1「结果应尽量保留自然段和换行」。逐条
//! `OcrLine` 的 `Text()` 再以 `\n` 连接，分段口径是引擎给的，不额外发明。
//!
//! 边长先按引擎自己报的 `MaxImageDimension` 缩一次：天花板是问来的而不是背下来的，
//! 而一张全屏 4K 的截图长边 3840 —— 超出去那一次调用会失败，用户在面板上读到的是一条
//! 天书似的原因。

use std::borrow::Cow;

use falcon_core::frame::Frame;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OcrError {
    #[error("这一台不是 Windows，系统里没有可调用文字识别引擎")]
    Unsupported,
    /// 没有引擎=当前显示语言没装"光学字符识别"这一选件，所以原因里连去哪儿补装一起说。
    #[error("本机没有可用的文字识别引擎：{0}")]
    NoEngine(String),
    #[error("这张图送不进识别引擎：{0}")]
    Input(String),
    #[error("识别没有跑完：{0}")]
    Recognize(String),
}

/// 识别一张图，回一段以 `\n` 分行的文本。
///
/// 空串是一个答案而不是故障：识别跑完了、图上没有字。面板据此说「图中未识别到文字」，
/// 而 [`OcrError`] 只留给真的没跑成的那几种（PRD §8.6「显示失败类型」）。
pub fn extract(frame: &Frame) -> Result<String, OcrError> {
    Ok(recognize(frame)?.join("\n"))
}

/// 识别并按引擎给出的行分段交出。 [`extract`] 也走这一条，分段口径因此只有一处。
#[cfg(windows)]
pub fn recognize(frame: &Frame) -> Result<Vec<String>, OcrError> {
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    // WinRT 的每一次调用都要求调用线程先进过套间。这一枚既可能落在 Qt 的主线程上，
    // 也可能落在识别自己的工作线程上，还可能撞上别的库已经初始化过的那一个：已经初始化
    // 返回 S_FALSE，套间模型不合返回 RPC_E_CHANGED_MODE，两种都不必在这里中止，后面的
    // 调用会自己给出真正的错。与 reveal.rs 待 CoInitializeEx 同一个道理 —— 这个返回值
    // 说的是套间状态而不是成败，丢掉它是有意的，不是把码吞了。
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };

    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|err| OcrError::NoEngine(err.to_string()))?;

    // 天花板问不到就当它不限，让后面那一次真调用去回答；不去猜一个数。本机量到的是
    // 10000（`.qoder/scratch/p146_ocr_test5.txt`），比一张全屏截图宽得多 —— 但这枚数
    // 是引擎给的，换一台机器换一个语言包就可能不是它，所以问而不是背。
    let ceiling = OcrEngine::MaxImageDimension().unwrap_or(u32::MAX);
    let source = fit(frame, ceiling)
        .map_err(|err| OcrError::Input(format!("长边缩进 {ceiling} 这一档天花板：{err}")))?;

    // 像素要变成一枚 WinRT 的 `IBuffer` 才进得了 `CreateCopyFromBuffer`。先试的是
    // `InMemoryRandomAccessStream` + `DataWriter` + `DetachBuffer`：实测那一枚交回的
    // 缓冲 `Capacity=218436` 而 `Length=0`（`.qoder/scratch/p146_ocr_test3.txt`），而
    // `CreateCopyFromBuffer` 判的是 `Length`，于是一场 `MF_E_INSUFFICIENT_BUFFER`。
    // `CreateFromByteArray` 一次调用就交回长度正确的缓冲，连流都不必建 —— 所以
    // §7.5 那句"不落临时文件"在这里是默认值，不是努力。
    let buffer = CryptographicBuffer::CreateFromByteArray(&to_bgra(&source.pixels))
        .map_err(input_at("造缓冲"))?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        source.width as i32,
        source.height as i32,
    )
    .map_err(input_at("按 Bgra8 造图"))?;

    let result =
        wait(engine.RecognizeAsync(&bitmap).map_err(onto_recognize)?).map_err(onto_recognize)?;
    let lines = result.Lines().map_err(onto_recognize)?;
    let count = lines.Size().map_err(onto_recognize)?;
    let mut out = Vec::with_capacity(count as usize);
    for index in 0..count {
        let line = lines.GetAt(index).map_err(onto_recognize)?;
        out.push(line.Text().map_err(onto_recognize)?.to_string());
    }
    Ok(out)
}

#[cfg(not(windows))]
pub fn recognize(_frame: &Frame) -> Result<Vec<String>, OcrError> {
    Err(OcrError::Unsupported)
}

/// 等一次 WinRT 异步调用跑完，取回它的结果。
///
/// `windows_future` 里那枚 `Async::join` 正是干这件事的，可 0.3.2 把它挂在私有的
/// `mod r#async` 上（`use r#async::*;` 而不是 `pub use`），外面够不着 —— 于是这里用
/// 公开的 `Status` 与 `GetResults` 自己等一回。
///
/// 选**问状态**而不是 `SetCompleted` 挂回调：完成回调会被派回**登记它的那个套间**，
/// 而挂完就在同一个线程上睡等它，在 STA（Qt 主线程那一枚）上是死锁 —— 回调排在本线程
/// 的消息队列里，本线程正睡着不泵。识别的活儿在系统线程池上跑，问状态不需要本线程泵
/// 消息，所以这条在两种套间里都站得住。5 ms 一问：一次识别几百毫秒，也就是几百问。
#[cfg(windows)]
fn wait<TResult: windows::core::RuntimeType + 'static>(
    operation: windows_future::IAsyncOperation<TResult>,
) -> windows::core::Result<TResult> {
    use windows_future::AsyncStatus;
    while operation.Status()? == AsyncStatus::Started {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    operation.GetResults()
}

/// 长边超出引擎天花板才缩，没超出就把同一张借出去。
///
/// 分岔留在这里而不是一路交给 [`Frame::thumbnail`]：那一枚对没超的图也答"原样"，
/// 而答法是 `clone` 一次 —— 一张全屏截图是三十来 MB 的像素，绝大多数截图正是没超的那一种。
#[cfg_attr(not(windows), allow(dead_code))]
fn fit(frame: &Frame, ceiling: u32) -> Result<Cow<'_, Frame>, falcon_core::frame::FrameError> {
    if frame.width.max(frame.height) <= ceiling {
        Ok(Cow::Borrowed(frame))
    } else {
        Ok(Cow::Owned(frame.thumbnail(ceiling)?))
    }
}

/// `Frame` 的 RGBA8 换到 WinRT `Bgra8` 要的字节序：动的只有头尾两枚。
///
/// 这一枚不挂在 `#[cfg(windows)]` 上：它一个 Windows 类型都不碰，而"字节序换对了"
/// 这件事在一台没有 Windows 的机器上同样可以问 —— 那里它只是没有调用点。
#[cfg_attr(not(windows), allow(dead_code))]
fn to_bgra(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect()
}

/// 失败挂在它发生的那一步上。§8.6 要"显示失败类型"：同样是"送不进识别引擎"，
/// 崩在"取回缓冲"和崩在"按 Bgra8 造图"上，用户该做的事不是一件。
#[cfg(windows)]
fn input_at(stage: &str) -> impl Fn(windows::core::Error) -> OcrError + use<'_> {
    move |err| OcrError::Input(format!("{stage}：{err}"))
}

#[cfg(windows)]
fn onto_recognize(err: windows::core::Error) -> OcrError {
    OcrError::Recognize(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 字节序错一位，OCR 拿到的就是一张红蓝互换的图，而它**照样**会回一段文字 —— 回的
    /// 是猜出来的。所以这里断言逐字节相等，不断言"能识别出来"。
    #[test]
    fn swapping_exchanges_red_and_blue_and_touches_nothing_else() {
        let rgba = [1u8, 2, 3, 255, 10, 20, 30, 0];
        assert_eq!(to_bgra(&rgba), vec![3, 2, 1, 255, 30, 20, 10, 0]);
    }

    /// 末尾多出来的一枚是别人的缓冲越界，不是半个像素：宁可整个丢掉，不按 4 的倍数
    /// 之外的长度交进引擎去（那一步会让 `CreateCopyFromBuffer` 的长宽算不上）。
    #[test]
    fn a_partial_pixel_at_the_end_is_not_swizzled() {
        assert_eq!(to_bgra(&[7, 8, 9]), Vec::<u8>::new());
    }

    /// 这一台不是 Windows 时回答"没有这一号"，而不是假装识别过并且图上没字。
    #[cfg(not(windows))]
    #[test]
    fn a_machine_without_windows_answers_that_it_has_no_engine() {
        let frame = Frame::filled(8, 8, [0, 0, 0, 255]).unwrap();
        let err = extract(&frame).unwrap_err();
        assert_eq!(err, OcrError::Unsupported);
    }

    /// 真机一趟：DirectWrite 排一行字到白底上，交进引擎，读回来的必须是那一行。
    ///
    /// 这一枚要的是**这台机器上的引擎真的能把像素读成字**，而这件事在语言包缺失的
    /// 机器上根本办不到，所以它挂在 `#[ignore]` 上、由门梯那一趟点名跑
    /// （`cargo test -p platform-windows -- --ignored`），不参与日常绿。
    #[cfg(windows)]
    #[test]
    #[ignore = "需要本机装有系统 OCR 语言包；由门梯点名跑"]
    fn letters_drawn_by_the_font_leg_come_back_as_letters() {
        use crate::glyphs::DirectWrite;
        use falcon_core::annotation::model::Style;
        use falcon_core::annotation::raster::Glyphs;

        let line = "FALCONSHOT 1234";
        let style = Style {
            font_size: 48,
            ..Style::default()
        };
        let ink = DirectWrite
            .ink(line, &style)
            .expect("DirectWrite must lay the line out");

        let margin = 24u32;
        let width = ink.width + margin * 2;
        let height = ink.height + margin * 2;
        let mut frame = Frame::filled(width, height, [255, 255, 255, 255]).unwrap();
        for y in 0..ink.height {
            for x in 0..ink.width {
                // coverage 是"这个像素有多少墨"，白底黑字就是按它插值；过半就当实心，
                // 与 glyphs::blit 取三枚子像素里最高的那一枚同一个口径。
                if ink.at(x, y) >= 128 {
                    let at = ((y + margin) as usize * width as usize + (x + margin) as usize) * 4;
                    frame.pixels[at..at + 4].copy_from_slice(&[0, 0, 0, 255]);
                }
            }
        }

        println!("ink {}x{}, frame {width}x{height}", ink.width, ink.height);
        let got = extract(&frame).expect("the system engine must answer");
        assert!(
            got.replace([' ', '\n'], "").contains("FALCONSHOT1234"),
            "asked for {line:?}, got {got:?}"
        );
    }

    /// 天花板比这张图宽：借出去的就是原来那一张，一次 `clone` 都没有 —— `Borrowed`
    /// 就是"没复制"这件事的凭据，而 4K 全屏截图正是这一支。
    #[test]
    fn a_frame_under_the_ceiling_is_handed_over_untouched() {
        let frame = Frame::filled(1920, 1080, [9, 9, 9, 255]).unwrap();
        let got = fit(&frame, 10_000).unwrap();
        assert!(matches!(got, Cow::Borrowed(_)), "缩了一次不必要的图");
        assert_eq!((got.width, got.height), (1920, 1080));
    }

    /// 天花板窄：长边正好顶到天花板上，短边按比例跟走，谁都不塌成 0。
    #[test]
    fn a_frame_over_the_ceiling_is_shrunk_onto_it() {
        let frame = Frame::filled(1000, 500, [9, 9, 9, 255]).unwrap();
        let got = fit(&frame, 100).unwrap();
        assert!(matches!(got, Cow::Owned(_)), "超出天花板却没缩");
        assert_eq!((got.width, got.height), (100, 50));
    }

    /// 顶边正好等于天花板：这不是"超出"，一寸都不该动。
    #[test]
    fn a_frame_exactly_the_ceiling_is_not_touched() {
        let frame = Frame::filled(100, 40, [9, 9, 9, 255]).unwrap();
        let got = fit(&frame, 100).unwrap();
        assert!(matches!(got, Cow::Borrowed(_)), "贴着天花板就缩了一次");
        assert_eq!((got.width, got.height), (100, 40));
    }
}
