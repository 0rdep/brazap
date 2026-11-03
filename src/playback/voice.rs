//! Decode WhatsApp's Ogg Opus into a bounded, temporary PCM stream for native
//! playback. Follows Rune's video/foreign.rs (OpusHead gain, pre-skip and final
//! granule trimming); the original compressed audio remains in SQLite.
use anyhow::{Context, Result, ensure};
use std::io::{Read, Seek, SeekFrom, Write};
use symphonia::core::{
    codecs::CODEC_TYPE_OPUS, errors::Error, formats::FormatOptions, io::MediaSourceStream,
    meta::MetadataOptions, probe::Hint,
};

pub(crate) fn prepare(original: tempfile::TempPath) -> Result<tempfile::TempPath> {
    procwatch::alloc::tag_thread("voice decode");
    let mut source = std::fs::File::open(&original)?;
    let mut magic = [0; 4];
    if source.read(&mut magic)? != 4 || (magic != *b"OggS" && magic != [0x1a, 0x45, 0xdf, 0xa3]) {
        return Ok(original);
    }
    source.rewind()?;
    let stream = MediaSourceStream::new(Box::new(source), Default::default());
    let mut demux = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            stream,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )?
        .format;
    let track = demux
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec == CODEC_TYPE_OPUS)
        .context("This voice message has no Opus track")?;
    let id = track.id;
    let head = track
        .codec_params
        .extra_data
        .as_deref()
        .context("Missing Opus header")?;
    ensure!(
        head.len() >= 19 && &head[..8] == b"OpusHead",
        "Invalid Opus voice header"
    );
    let channels = head[9] as usize;
    ensure!(
        head[18] == 0 && (1..=2).contains(&channels),
        "Unsupported voice channel layout"
    );
    let pre_skip = u16::from_le_bytes([head[10], head[11]]) as usize;
    let mut skip = pre_skip;
    let gain = 10f32.powf(i16::from_le_bytes([head[16], head[17]]) as f32 / 256.0 / 20.0);
    let keep = if magic == *b"OggS" {
        track
            .codec_params
            .n_frames
            .map(|n| n.saturating_sub(pre_skip as u64) * channels as u64)
    } else {
        None
    };
    let mut decoder = opuscule::Decoder::new(
        opuscule::SampleRate::Hz48000,
        if channels == 1 {
            opuscule::Channels::Mono
        } else {
            opuscule::Channels::Stereo
        },
    );
    let mut buffer = vec![0f32; 5760 * channels];
    let mut encoded = Vec::with_capacity(buffer.len() * 2);
    let mut file = tempfile::Builder::new()
        .prefix("brazap-voice-")
        .suffix(".wav")
        .tempfile()?;
    file.write_all(&[0; 44])?;
    let mut samples = 0u64;
    loop {
        let packet = match demux.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != id {
            continue;
        }
        let frames = decoder
            .decode(Some(&packet.data), &mut buffer, false)
            .map_err(|e| anyhow::anyhow!("Opus decode failed: {e}"))?;
        let start = skip.min(frames);
        skip -= start;
        encoded.clear();
        for sample in &buffer[start * channels..frames * channels] {
            let value = ((sample * gain).clamp(-1.0, 1.0) * 32767.0) as i16;
            encoded.extend_from_slice(&value.to_le_bytes());
        }
        samples += (encoded.len() / 2) as u64;
        ensure!(
            samples * 2 <= 256 * 1024 * 1024,
            "Voice message is too long to decode"
        );
        file.write_all(&encoded)?;
    }
    let samples = keep.map_or(samples, |n| n.min(samples));
    ensure!(samples > 0, "Voice message contains no audio");
    let bytes = (samples * 2) as u32;
    file.as_file_mut().set_len(44 + bytes as u64)?;
    file.seek(SeekFrom::Start(0))?;
    let channels = channels as u16;
    file.write_all(b"RIFF")?;
    file.write_all(&(bytes + 36).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&channels.to_le_bytes())?;
    file.write_all(&48_000u32.to_le_bytes())?;
    file.write_all(&(48_000u32 * channels as u32 * 2).to_le_bytes())?;
    file.write_all(&(channels * 2).to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())?;
    file.flush()?;
    // Close the demuxer's source handle before deleting the compressed temp copy.
    drop(demux);
    let _ = original.close();
    Ok(file.into_temp_path())
}
