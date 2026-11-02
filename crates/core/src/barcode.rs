//! PRD §5.6.2（二维码）与 §5.6.3（条形码）共用的那一只手：一张 RGBA 帧进去，读到的码出来。
//!
//! 为什么这一层只交灰度：rxing 的解码路径吃的是亮度缓冲（计划 §2.1 钉它那一行写的就是「吃内存
//! 亮度缓冲」），所以本模块做的唯一一件图像学的事是 Rec.601 加权 —— 不放大、不旋转、不二次
//! 采样，那些都归调用者（选区已经剪好，贴图已经渲染好）。
//!
//! 为什么是 `rxing` 而不是 `zedbar`：许可证。计划 §2.1 那一行点名要避开 LGPL-3.0，而本档是
//! 打包进 MSIX 分发的产品。

use crate::frame::Frame;

/// 一枚读出来的码。`format` 是**类型名**，因为 §5.6.3 步骤 3 要「解析条码类型和编码内容」——
/// 只交内容的那一只手答不出这一问。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    pub text: String,
    pub format: String,
}

#[derive(Debug, thiserror::Error)]
pub enum BarcodeError {
    /// §5.6.2 的安全规则要「对无法识别或损坏的二维码显示明确提示」，所以「这一张里一枚也没有」
    /// 自成一格，而不是混进解码失败那一格：同一句话说不清是图不对还是引擎不对。
    #[error("没在这张图里找到二维码或条形码。")]
    NotFound,
    /// 引擎自己失败 —— 损坏的码、读不动的字符集、尺寸退化的一帧。原文带回来，这一层不编故事。
    #[error("解码失败：{0}")]
    Decode(String),
}

/// 读出帧里**所有**的码。多枚是一等情形而不是意外：`detect_multiple_in_luma` 会把一张图切四块
/// 递归找，所以同一枚码被读回两次是它的正常行为 ⇒ 同名同内容的作一次差，界面才不会出现两行
/// 一模一样的「复制」。
pub fn read(frame: &Frame) -> Result<Vec<Code>, BarcodeError> {
    let luma = to_luma(frame);
    let found = match rxing::helpers::detect_multiple_in_luma(luma, frame.width, frame.height) {
        Ok(list) => list,
        Err(rxing::Exceptions::NotFoundException(_)) => return Err(BarcodeError::NotFound),
        Err(e) => return Err(BarcodeError::Decode(e.to_string())),
    };
    if found.is_empty() {
        return Err(BarcodeError::NotFound);
    }

    let mut codes: Vec<Code> = Vec::new();
    for one in found {
        let code = Code {
            text: one.getText().to_string(),
            format: format!("{:?}", one.getBarcodeFormat()),
        };
        if !codes.contains(&code) {
            codes.push(code);
        }
    }
    Ok(codes)
}

/// Rec.601 的整数形：`(299 R + 587 G + 114 B) / 1000`。
///
/// 不用 `image` 的 `to_luma`：那条路要把帧摊成 `GrayImage`，而本档的帧一直是自己那枚
/// `Vec<u8>`（计划 §2.1 那行关掉 rxing 的 `image` 特征，正是为了不再拖一份 `image = =0.25.8`
/// 进来和 workspace 的 `^0.25.10` 撞）。
fn to_luma(frame: &Frame) -> Vec<u8> {
    frame
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| (u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000)
        .map(|v| v as u8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;
    use rxing::{BarcodeFormat, MultiFormatWriter, Writer};

    /// 把一枚写出来的位图铺成帧：暗模块 = 黑、亮模块 = 白，四周留静区（§5.6.2 那一节的码都是
    /// 这么要求的，而 rxing 自己也要求 quiet zone）。
    fn paint(matrix: &rxing::common::BitMatrix, quiet: u32) -> Frame {
        let (mw, mh) = (matrix.width(), matrix.height());
        let w = mw + quiet * 2;
        let h = mh + quiet * 2;
        let mut frame = Frame::new(w, h).expect("frame of a painted code");
        for y in 0..h {
            for x in 0..w {
                let dark = x >= quiet
                    && y >= quiet
                    && x - quiet < mw
                    && y - quiet < mh
                    && matrix.get(x - quiet, y - quiet);
                let v = if dark { 0 } else { 255 };
                let off = ((y * w + x) * 4) as usize;
                frame.pixels[off..off + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        frame
    }

    #[test]
    fn a_written_qr_comes_back_as_a_qr() {
        let matrix = MultiFormatWriter
            .encode(
                "https://falcon.example/a?x=1",
                &BarcodeFormat::QR_CODE,
                160,
                160,
            )
            .expect("writing a QR of this size");
        let codes = read(&paint(&matrix, 8)).expect("the code we just painted must be found");
        assert_eq!(codes.len(), 1, "one code in, one code out: {codes:?}");
        assert_eq!(codes[0].text, "https://falcon.example/a?x=1");
        assert_eq!(codes[0].format, "QR_CODE");
    }

    /// §5.6.1 那条 OCR 腿的欠账是「中文没证」；这一条本轮就把它证了：内容走 UTF-8 的码，
    /// 汉字进去汉字出来。
    #[test]
    fn a_chinese_qr_comes_back_as_chinese() {
        let matrix = MultiFormatWriter
            .encode(" Falcon 截图 中文 123", &BarcodeFormat::QR_CODE, 160, 160)
            .expect("writing a QR of Chinese text");
        let codes = read(&paint(&matrix, 8)).expect("the Chinese code must be found");
        assert_eq!(
            codes[0].text, " Falcon 截图 中文 123",
            "got {:?}",
            codes[0].text
        );
    }

    /// §5.6.3 步骤 3 要的是「类型和内容」两样，所以一维码这一趟单独看类型名。
    #[test]
    fn a_one_d_code_reports_its_own_type() {
        let matrix = MultiFormatWriter
            .encode("12345678", &BarcodeFormat::CODE_128, 300, 80)
            .expect("writing a Code 128");
        let codes = read(&paint(&matrix, 12)).expect("the bar code must be found");
        assert_eq!(codes[0].text, "12345678");
        assert_eq!(codes[0].format, "CODE_128");
    }

    #[test]
    fn a_blank_frame_says_there_is_no_code() {
        let white = Frame::filled(120, 60, [255, 255, 255, 255]).expect("a white frame");
        assert!(matches!(read(&white), Err(BarcodeError::NotFound)));
    }

    #[test]
    fn a_degenerate_frame_is_an_error_not_a_panic() {
        // 这一枚单测只主张一件事：退化的一帧交回的是 Err 而不是把 panic 传给调用者。它**不**主张
        // 错误落在哪一格 —— rxing 对 0 尺寸交回的是 IllegalArgumentException 而不是
        // NotFoundException，那一格归 Decode。
        let empty = Frame {
            width: 0,
            height: 0,
            pixels: Vec::new(),
        };
        assert!(matches!(
            read(&empty),
            Err(BarcodeError::Decode(_) | BarcodeError::NotFound)
        ));
    }

    #[test]
    fn the_luma_weighting_puts_red_and_green_apart() {
        // 纯红、纯绿、纯蓝各 255 的亮度必须是 76 / 149 / 29 —— Rec.601 的那三枚系数就是它们的
        // 读数，这一枚单测看的是「权重没写反」（绿的权最重，所以它最高）。
        let mut frame = Frame::new(3, 1).expect("three pixels");
        for (i, px) in [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]]
            .iter()
            .enumerate()
        {
            frame.pixels[i * 4..i * 4 + 4].copy_from_slice(px);
        }
        assert_eq!(to_luma(&frame), vec![76, 149, 29], "Rec.601 integer form");
    }
}
