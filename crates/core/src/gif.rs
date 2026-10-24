//! §5.9.14: the decoded GIF - every frame the file carries, and the pace it
//! asked for.
//!
//! Two things live here and nowhere else:
//!
//! * **The pixels.** `image`'s GIF decoder blends each frame against the canvas
//!   the previous one left, so an optimised GIF whose later frames only touch a
//!   corner still arrives as full-canvas pictures. That is what lets
//!   [`crate::pin::PlayState`] be nothing but an index into this list.
//! * **The budget.** A clip is a *list* of pictures, so the single-picture guard
//!   in [`encode::DECODE_MAX_PIXELS`] does not cover it: three frames at that
//!   limit would be 768 MB. The budget here is on the total, and it is charged
//!   as frames arrive rather than trusted from a header count.
//!
//! Deliberately absent: the loop count. Nothing on screen repeats a clip on cue
//! yet, so storing it would be a number with no reader - the same debt §5.20.1
//! is carrying for four `card.*` keys, recorded here so it is not made twice.

use std::io::{Cursor, Read};
use std::path::Path;

use image::codecs::gif::GifDecoder;
use image::AnimationDecoder;
use image::Frame as AnimFrame;
use thiserror::Error;

use crate::encode;
use crate::frame::Frame;

/// `GIF87a` and `GIF89a` both start with this. Four bytes is the shortest header
/// that cannot belong to another format, which is what lets [`decode_file`]
/// answer "not a GIF" without reading the file.
pub const GIF_MAGIC: &[u8; 4] = b"GIF8";
/// All the frames of one clip together may not outweigh a single still.
pub const GIF_MAX_TOTAL_PIXELS: u64 = encode::DECODE_MAX_PIXELS;
/// Past this many frames a clip is refused rather than streamed: it is already
/// more than a minute of 30 fps, and every one of them is a full canvas.
pub const GIF_MAX_FRAMES: usize = 2000;

#[derive(Debug, Error)]
pub enum GifError {
    #[error("codec failed: {0}")]
    Codec(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("the GIF carries no frames")]
    Empty,
    #[error("a {0}x{1} frame would take the clip over its {2} pixel budget")]
    TooLarge(u32, u32, u64),
    #[error("a clip may not carry more than {0} frames")]
    TooManyFrames(usize),
}

/// One decoded GIF: `frames[0]` is what the pin shows before the clock moves,
/// and `delays_ms[i]` is how long frame `i` wants to be on screen.
#[derive(Clone, Debug)]
pub struct GifClip {
    pub frames: Vec<Frame>,
    pub delays_ms: Vec<u32>,
}

impl GifClip {
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// The frame the clock is on. `None` when the index has nothing behind it -
    /// which is what a restored session whose pixels went away looks like.
    pub fn frame_at(&self, at: usize) -> Option<&Frame> {
        self.frames.get(at)
    }
}

/// What the header says, and nothing else.
pub fn is_gif(bytes: &[u8]) -> bool {
    bytes.starts_with(GIF_MAGIC)
}

pub fn decode(bytes: &[u8]) -> Result<GifClip, GifError> {
    if !is_gif(bytes) {
        return Err(GifError::Codec(
            "the bytes do not start with a GIF header".into(),
        ));
    }
    let decoder = GifDecoder::new(Cursor::new(bytes)).map_err(codec)?;
    let mut frames: Vec<Frame> = Vec::new();
    let mut delays_ms: Vec<u32> = Vec::new();
    let mut spent: u64 = 0;
    for frame in decoder.into_frames() {
        if frames.len() >= GIF_MAX_FRAMES {
            return Err(GifError::TooManyFrames(GIF_MAX_FRAMES));
        }
        let frame = frame.map_err(codec)?;
        // The delay is read before the frame is taken apart for its pixels.
        let pace = delay_ms(&frame);
        let frame = frame.into_buffer();
        spent = charge(spent, frame.width(), frame.height())?;
        delays_ms.push(pace);
        frames.push(Frame::from_image(frame));
    }
    if frames.is_empty() {
        return Err(GifError::Empty);
    }
    Ok(GifClip { frames, delays_ms })
}

/// Decode a file, answering `Ok(None)` when it is not a GIF at all. The header
/// is read before anything is decoded, so pasting a PNG costs four bytes rather
/// than a second full read of the file.
pub fn decode_file(path: &Path) -> Result<Option<GifClip>, GifError> {
    let mut head = [0u8; GIF_MAGIC.len()];
    let mut file = std::fs::File::open(path)?;
    let got = file.read(&mut head)?;
    if got < GIF_MAGIC.len() || !is_gif(&head) {
        return Ok(None);
    }
    drop(file);
    decode(&std::fs::read(path)?).map(Some)
}

/// The file's own pace for one frame, in milliseconds. A GIF stores its delay in
/// hundredths of a second, so a frame that asked for less than 10 ms arrives as
/// zero - and zero is the same "the file says nothing" as a missing entry.
fn delay_ms(frame: &AnimFrame) -> u32 {
    let (numer, denom) = frame.delay().numer_denom_ms();
    numer.checked_div(denom).unwrap_or(0)
}

/// The budget, one frame at a time. Split out because the arithmetic is what a
/// test can actually pin; the loop above only decides when to call it.
fn charge(spent: u64, width: u32, height: u32) -> Result<u64, GifError> {
    let next = spent.saturating_add(u64::from(width) * u64::from(height));
    if next > GIF_MAX_TOTAL_PIXELS {
        return Err(GifError::TooLarge(width, height, GIF_MAX_TOTAL_PIXELS));
    }
    Ok(next)
}

fn codec<E: std::fmt::Display>(e: E) -> GifError {
    GifError::Codec(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::GifEncoder;
    use image::Frame as Anim;
    use image::{Delay, ImageBuffer, Rgba};

    /// The colours are black and white on purpose: GIF palettises, and those two
    /// survive the trip exactly, so a test can compare pixels and not approximations.
    fn canvas(w: u32, h: u32, shade: u8) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
        ImageBuffer::from_pixel(w, h, Rgba([shade, shade, shade, 255]))
    }

    /// A clip of `paces.len()` frames, each held for `paces[i]` milliseconds.
    fn gif_bytes(paces: &[u32]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        {
            let mut enc = GifEncoder::new(&mut out);
            for (i, &pace) in paces.iter().enumerate() {
                let shade = if i % 2 == 0 { 0 } else { 255 };
                let frame = Anim::from_parts(
                    canvas(8, 6, shade),
                    0,
                    0,
                    Delay::from_numer_denom_ms(pace, 1),
                );
                enc.encode_frame(frame).expect("frame");
            }
        }
        out.into_inner()
    }

    #[test]
    fn a_clip_decodes_into_its_own_frames_at_its_own_pace() {
        let clip = decode(&gif_bytes(&[120, 30, 60])).expect("decode");
        assert_eq!(clip.frame_count(), 3);
        assert_eq!(clip.delays_ms, vec![120, 30, 60]);
        for f in &clip.frames {
            assert_eq!((f.width, f.height), (8, 6));
            assert_eq!(f.pixels.len(), 8 * 6 * 4);
        }
        // Every frame is the whole canvas, not a patch: index 0 is dark, 1 light.
        assert_eq!(&clip.frames[0].pixels[0..4], &[0, 0, 0, 255]);
        assert_eq!(&clip.frames[1].pixels[0..4], &[255, 255, 255, 255]);
        assert_eq!(&clip.frames[2].pixels[0..4], &[0, 0, 0, 255]);
        assert!(clip.frame_at(2).is_some());
        assert!(clip.frame_at(3).is_none());
    }

    #[test]
    fn a_still_gif_is_one_frame_and_no_pace_to_play() {
        let clip = decode(&gif_bytes(&[50])).expect("decode");
        assert_eq!(clip.frame_count(), 1);
        assert_eq!(clip.delays_ms, vec![50]);
    }

    #[test]
    fn a_delay_below_the_file_s_own_resolution_arrives_as_zero() {
        // GIF stores hundredths of a second: 5 ms cannot be written, so it must
        // read back as the "nothing declared" that PlayState then floors.
        let clip = decode(&gif_bytes(&[5, 5])).expect("decode");
        assert_eq!(clip.delays_ms, vec![0, 0]);
    }

    #[test]
    fn the_header_is_the_thing_that_decides_who_is_read() {
        let png = encode::encode(
            &Frame::filled(4, 4, [7, 8, 9, 255]).expect("frame"),
            &encode::EncodeOptions {
                format: encode::Format::Png,
                ..Default::default()
            },
        )
        .expect("png");
        let gif = gif_bytes(&[10]);
        assert!(is_gif(&gif));
        assert!(!is_gif(&png));
        assert!(!is_gif(b"GIF"));
        assert!(decode(&png).is_err());
        assert!(decode(b"not a gif at all").is_err());
    }

    #[test]
    fn a_non_gif_file_is_answered_as_none_without_being_decoded() {
        let dir = std::env::temp_dir().join(format!("falconshot-gif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let png = dir.join("still.png");
        let frame = Frame::filled(4, 4, [7, 8, 9, 255]).expect("frame");
        let opt = encode::EncodeOptions {
            format: encode::Format::Png,
            ..Default::default()
        };
        encode::save(&frame, &png, &opt).expect("save");
        assert!(decode_file(&png).expect("no error").is_none());

        let gif = dir.join("clip.gif");
        std::fs::write(&gif, gif_bytes(&[40, 40])).expect("write");
        let clip = decode_file(&gif).expect("decode").expect("a clip");
        assert_eq!(clip.frame_count(), 2);
        assert_eq!(clip.delays_ms, vec![40, 40]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_budget_is_the_whole_clip_not_one_frame() {
        let full = GIF_MAX_TOTAL_PIXELS;
        assert_eq!(charge(0, 1024, 1024).expect("first"), 1024 * 1024);
        assert_eq!(charge(full - 1, 1, 1).expect("last pixel"), full);
        assert!(matches!(
            charge(full, 1, 1),
            Err(GifError::TooLarge(1, 1, _))
        ));
        // One frame alone may not be turned into an unbounded total.
        assert!(charge(0, u32::MAX, u32::MAX).is_err());
    }
}
