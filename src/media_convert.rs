//! Converts media for stickers and the clipboard: frames from WhatsApp videos and GIFs
//! (H.264 in MP4), animated WebP stickers, and GIF exports.
use anyhow::{Context, Result, bail, ensure};
use image::{
    AnimationDecoder, Delay, Frame, RgbaImage,
    codecs::{gif::GifEncoder, webp::WebPDecoder},
    imageops::{self, FilterType},
};
use std::io::Cursor;

/// WhatsApp's sticker canvas.
pub const STICKER_SIDE: u32 = 512;
/// WhatsApp refuses larger stickers.
const STATIC_STICKER_LIMIT: usize = 100 * 1024;
const ANIMATED_STICKER_LIMIT: usize = 500 * 1024;
/// Animated stickers loop, so only the opening seconds of a longer video are used.
const STICKER_SECONDS: f64 = 8.0;
const GIF_SECONDS: f64 = 30.0;

/// Decodes a video's frames in display order, resized to fit `max_side`, and passes each with
/// its time in milliseconds to `frame` until it returns `false` or `max_seconds` pass.
pub fn video_frames(
    mp4: &[u8],
    max_side: u32,
    max_seconds: f64,
    mut frame: impl FnMut(RgbaImage, u32) -> Result<bool>,
) -> Result<()> {
    let mut reader = mp4::Mp4Reader::read_header(Cursor::new(mp4), mp4.len() as u64)
        .context("This video could not be read")?;
    let (track_id, track) = reader
        .tracks()
        .iter()
        .find(|(_, track)| matches!(track.media_type(), Ok(mp4::MediaType::H264)))
        .map(|(id, track)| (*id, track))
        .context("Only H.264 videos can be converted")?;
    let avc = track
        .trak
        .mdia
        .minf
        .stbl
        .stsd
        .avc1
        .as_ref()
        .context("This video has no H.264 configuration")?;
    let length_size = usize::from(avc.avcc.length_size_minus_one & 3) + 1;
    let mut header = Vec::new();
    for nal in avc
        .avcc
        .sequence_parameter_sets
        .iter()
        .chain(&avc.avcc.picture_parameter_sets)
    {
        header.extend_from_slice(&[0, 0, 0, 1]);
        header.extend_from_slice(&nal.bytes);
    }
    let matrix = &track.trak.tkhd.matrix;
    let rotation = match (matrix.a, matrix.b, matrix.c, matrix.d) {
        (0, b, c, 0) if b > 0 && c < 0 => 90,
        (a, 0, 0, d) if a < 0 && d < 0 => 180,
        (0, b, c, 0) if b < 0 && c > 0 => 270,
        _ => 0,
    };
    let timescale = f64::from(track.timescale().max(1));
    let count = track.sample_count();
    // The decoder returns frames in display order, while samples are stored in decode order.
    let mut times = Vec::with_capacity(count as usize);
    let mut samples = Vec::with_capacity(count as usize);
    for id in 1..=count {
        let Some(sample) = reader.read_sample(track_id, id)? else {
            continue;
        };
        let time = (sample.start_time as f64 + f64::from(sample.rendering_offset)) / timescale;
        times.push(time.max(0.0));
        samples.push(sample.bytes);
    }
    times.sort_by(f64::total_cmp);
    // Flushing after every packet breaks videos with B-frames; frames are flushed at the end.
    let mut decoder = openh264::decoder::Decoder::with_api_config(
        openh264::OpenH264API::from_source(),
        openh264::decoder::DecoderConfig::new()
            .flush_after_decode(openh264::decoder::Flush::NoFlush),
    )?;
    let mut shown = 0usize;
    let mut emit = |yuv: &openh264::decoder::DecodedYUV<'_>, shown: &mut usize| -> Result<bool> {
        use openh264::formats::YUVSource;
        let Some(&time) = times.get(*shown) else {
            return Ok(false);
        };
        *shown += 1;
        if time > max_seconds {
            return Ok(false);
        }
        let (width, height) = yuv.dimensions();
        ensure!(
            width > 0 && height > 0 && width <= 4096 && height <= 4096,
            "This video is too large to convert"
        );
        let mut rgba = vec![0; width * height * 4];
        yuv.write_rgba8(&mut rgba);
        let image = RgbaImage::from_raw(width as u32, height as u32, rgba)
            .context("Invalid video frame")?;
        let image = match rotation {
            90 => imageops::rotate90(&image),
            180 => imageops::rotate180(&image),
            270 => imageops::rotate270(&image),
            _ => image,
        };
        frame(fit(&image, max_side), (time * 1000.0) as u32)
    };
    let mut packet = Vec::new();
    for (index, sample) in samples.iter().enumerate() {
        packet.clear();
        if index == 0 {
            packet.extend_from_slice(&header);
        }
        let mut rest = &sample[..];
        while rest.len() > length_size {
            let size = rest[..length_size]
                .iter()
                .fold(0usize, |size, byte| size << 8 | usize::from(*byte));
            rest = &rest[length_size..];
            ensure!(size <= rest.len(), "This video is damaged");
            packet.extend_from_slice(&[0, 0, 0, 1]);
            packet.extend_from_slice(&rest[..size]);
            rest = &rest[size..];
        }
        // A damaged frame is skipped; the next keyframe recovers the picture.
        if let Ok(Some(yuv)) = decoder.decode(&packet)
            && !emit(&yuv, &mut shown)?
        {
            return Ok(());
        }
    }
    for yuv in decoder.flush_remaining().unwrap_or_default() {
        if !emit(&yuv, &mut shown)? {
            break;
        }
    }
    ensure!(shown > 0, "This video has no frames that could be decoded");
    Ok(())
}

/// The first frame of a video as PNG bytes, fitting within `max_side`.
pub fn first_frame_png(mp4: &[u8], max_side: u32) -> Result<Vec<u8>> {
    let mut first = None;
    video_frames(mp4, max_side, f64::MAX, |image, _| {
        first = Some(image);
        Ok(false)
    })?;
    let mut image = first.context("This video has no frames that could be decoded")?;
    // Video frames are opaque; the decoder leaves alpha unset on some platforms.
    for pixel in image.pixels_mut() {
        pixel[3] = 255;
    }
    let mut png = Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png)?;
    Ok(png.into_inner())
}

/// Scales an image down to fit within `max_side`, keeping its proportions.
fn fit(image: &RgbaImage, max_side: u32) -> RgbaImage {
    let (width, height) = image.dimensions();
    if width <= max_side && height <= max_side {
        return image.clone();
    }
    let scale = f64::from(max_side) / f64::from(width.max(height));
    let size = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
    imageops::resize(image, size(width), size(height), FilterType::Triangle)
}

/// Centers an image on the transparent square sticker canvas.
fn canvas(image: &RgbaImage) -> RgbaImage {
    let image = fit(image, STICKER_SIDE);
    let mut canvas = RgbaImage::new(STICKER_SIDE, STICKER_SIDE);
    let x = (STICKER_SIDE - image.width()) / 2;
    let y = (STICKER_SIDE - image.height()) / 2;
    imageops::overlay(&mut canvas, &image, i64::from(x), i64::from(y));
    canvas
}

/// A still WebP sticker from a photo, within WhatsApp's size limit.
pub fn image_sticker(bytes: &[u8]) -> Result<Vec<u8>> {
    let image = image::load_from_memory(bytes)
        .context("This image could not be read")?
        .into_rgba8();
    let canvas = canvas(&image);
    for quality in [85.0, 70.0, 55.0, 40.0, 25.0] {
        let webp = encode_still(&canvas, quality)?;
        if webp.len() <= STATIC_STICKER_LIMIT {
            return Ok(webp);
        }
    }
    bail!("This image is too detailed to fit in a sticker")
}

fn encode_still(image: &RgbaImage, quality: f32) -> Result<Vec<u8>> {
    let mut encoder = webp_animation::Encoder::new_with_options(
        image.dimensions(),
        webp_animation::EncoderOptions {
            encoding_config: Some(webp_animation::EncodingConfig::new_lossy(quality)),
            ..Default::default()
        },
    )
    .map_err(|error| anyhow::anyhow!("Could not create the sticker: {error:?}"))?;
    encoder
        .add_frame(image.as_raw(), 0)
        .map_err(|error| anyhow::anyhow!("Could not create the sticker: {error:?}"))?;
    let webp = encoder
        .finalize(100)
        .map_err(|error| anyhow::anyhow!("Could not create the sticker: {error:?}"))?;
    // A single frame is stored without the animation chunks, as a still WebP.
    Ok(webp.to_vec())
}

/// An animated WebP sticker from a video or GIF, within WhatsApp's size limit.
pub fn video_sticker(mp4: &[u8]) -> Result<Vec<u8>> {
    let mut frames = Vec::new();
    // Ten frames a second keeps motion smooth while the sticker stays small.
    let mut next = 0u32;
    video_frames(mp4, STICKER_SIDE, STICKER_SECONDS, |image, at| {
        if at >= next {
            frames.push((canvas(&image), at));
            next = at + 100;
        }
        Ok(true)
    })?;
    ensure!(!frames.is_empty(), "This video has no frames");
    if frames.len() == 1 {
        return image_sticker_from(&frames[0].0);
    }
    for step in [1, 2, 3] {
        let frames: Vec<_> = frames.iter().step_by(step).collect();
        for quality in [70.0, 55.0, 40.0, 28.0] {
            let webp = encode_animation(&frames, quality)?;
            if webp.len() <= ANIMATED_STICKER_LIMIT {
                return Ok(webp);
            }
        }
    }
    bail!("This video is too detailed to fit in a sticker")
}

fn image_sticker_from(image: &RgbaImage) -> Result<Vec<u8>> {
    for quality in [85.0, 70.0, 55.0, 40.0, 25.0] {
        let webp = encode_still(image, quality)?;
        if webp.len() <= STATIC_STICKER_LIMIT {
            return Ok(webp);
        }
    }
    bail!("This image is too detailed to fit in a sticker")
}

fn encode_animation(frames: &[&(RgbaImage, u32)], quality: f32) -> Result<Vec<u8>> {
    let error = |error| anyhow::anyhow!("Could not create the sticker: {error:?}");
    let mut encoder = webp_animation::Encoder::new_with_options(
        (STICKER_SIDE, STICKER_SIDE),
        webp_animation::EncoderOptions {
            encoding_config: Some(webp_animation::EncodingConfig::new_lossy(quality)),
            ..Default::default()
        },
    )
    .map_err(error)?;
    let start = frames[0].1;
    for (image, at) in frames {
        encoder
            .add_frame(image.as_raw(), (at - start) as i32)
            .map_err(error)?;
    }
    let last = frames[frames.len() - 1].1 - start;
    let step = if frames.len() > 1 {
        last / (frames.len() as u32 - 1)
    } else {
        100
    };
    Ok(encoder
        .finalize((last + step.max(20)) as i32)
        .map_err(error)?
        .to_vec())
}

/// Whether WebP bytes hold an animation.
pub fn animated_webp(bytes: &[u8]) -> bool {
    crate::assets::animated_webp(bytes)
}

/// A GIF from an animated WebP sticker, keeping its timing and transparency. Stickers are
/// shown small, and GIFs at full sticker size are several megabytes.
pub fn webp_to_gif(webp: &[u8]) -> Result<Vec<u8>> {
    let frames = WebPDecoder::new(Cursor::new(webp))?
        .into_frames()
        .collect_frames()
        .context("This sticker could not be read")?
        .into_iter()
        .map(|frame| {
            let delay = frame.delay();
            Frame::from_parts(fit(&frame.into_buffer(), 320), 0, 0, delay)
        });
    let mut gif = Vec::new();
    {
        let mut encoder = GifEncoder::new_with_speed(&mut gif, 10);
        encoder.set_repeat(image::codecs::gif::Repeat::Infinite)?;
        encoder.encode_frames(frames)?;
    }
    Ok(gif)
}

/// A GIF from a WhatsApp GIF, which is sent as a short video.
pub fn video_to_gif(mp4: &[u8]) -> Result<Vec<u8>> {
    let mut gif = Vec::new();
    {
        let mut encoder = GifEncoder::new_with_speed(&mut gif, 10);
        encoder.set_repeat(image::codecs::gif::Repeat::Infinite)?;
        // GIF delays are in hundredths of a second; 15 frames a second keeps files small.
        let mut pending: Option<(RgbaImage, u32)> = None;
        let mut next = 0u32;
        video_frames(mp4, 480, GIF_SECONDS, |image, at| {
            if at < next {
                return Ok(true);
            }
            next = at + 66;
            if let Some((previous, started)) = pending.replace((image, at)) {
                let delay = Delay::from_numer_denom_ms((at - started).max(20), 1);
                encoder.encode_frame(Frame::from_parts(previous, 0, 0, delay))?;
            }
            Ok(true)
        })?;
        if let Some((last, _)) = pending {
            let delay = Delay::from_numer_denom_ms(66, 1);
            encoder.encode_frame(Frame::from_parts(last, 0, 0, delay))?;
        }
    }
    Ok(gif)
}
