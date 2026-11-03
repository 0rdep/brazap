//! A separate SQLite profile for UI development; never constructs a WhatsApp client.
use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageBuffer, Rgba};
use sha2::{Digest, Sha256};
use std::io::Cursor;
use whatsapp_rust::{prelude::*, waproto::buffa::MessageField};

use crate::{
    config::Config,
    model::{Message, now},
    service::Service,
    storage::{Archive, outgoing, project_message},
};

pub(crate) const OWN: &str = "12025550100@s.whatsapp.net";
const ANA: &str = "12025550107@s.whatsapp.net";
const LUCAS: &str = "12025550114@s.whatsapp.net";
const DESIGN: &str = "120363000000001@g.us";

pub(crate) fn prepare(config: &Config) -> Result<()> {
    let marker = config.data_dir.join(".brazap-demo");
    ensure!(
        !config.database().exists() || marker.exists(),
        "Demo mode requires an empty directory or an existing demo profile. Your WhatsApp data was not changed."
    );
    std::fs::write(marker, "Brazap offline demo profile\n")?;
    Ok(())
}

fn picture() -> Result<Vec<u8>> {
    let pixels = ImageBuffer::from_fn(640, 360, |x, y| {
        let horizon = 225.0 + 32.0 * (x as f32 / 85.0).sin();
        if (y as f32) > horizon {
            Rgba([27, 68 + (y / 9) as u8, 75, 255])
        } else if (x as i32 - 465).pow(2) + (y as i32 - 95).pow(2) < 35 * 35 {
            Rgba([254, 223, 151, 255])
        } else {
            Rgba([
                68 + (y / 4) as u8,
                107 + (y / 5) as u8,
                133 + (y / 6) as u8,
                255,
            ])
        }
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(pixels).write_to(&mut bytes, image::ImageFormat::Png)?;
    Ok(bytes.into_inner())
}

fn voice() -> Vec<u8> {
    let sample_rate = 16000u32;
    let samples = sample_rate * 8;
    let size = samples * 2;
    let mut bytes = Vec::with_capacity(size as usize + 44);
    bytes.extend(b"RIFF");
    bytes.extend((size + 36).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(sample_rate.to_le_bytes());
    bytes.extend((sample_rate * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(size.to_le_bytes());
    for i in 0..samples {
        let t = i as f32 / sample_rate as f32;
        let envelope = (t * 2.2).sin().abs() * ((8.0 - t) / 8.0);
        let value = ((t * 220.0 * std::f32::consts::TAU).sin() * envelope * 1200.0) as i16;
        bytes.extend(value.to_le_bytes());
    }
    bytes
}

pub(crate) async fn seed(archive: &Archive) -> Result<()> {
    if archive.preference("demo_seed_v1".into()).await?.is_some() {
        return Ok(());
    }
    let base = now() - 1200;
    for (jid, name) in [
        (ANA, "Ana Martins ✨"),
        (LUCAS, "Lucas Ferreira 👨🏽‍💻"),
        ("12025550119@s.whatsapp.net", "Sofia Costa"),
    ] {
        archive
            .contact(jid.into(), name.into(), "{}".into(), true)
            .await?;
    }
    for (jid, name, text) in [
        (DESIGN, "Brazap Design 🇧🇷", "Looks great. Let’s ship it! 💚"),
        (ANA, "Ana Martins ✨", "Sent you a voice message"),
        (
            "120363000000002@g.us",
            "Weekend plans",
            "Coffee on Saturday? ☕️",
        ),
        (LUCAS, "Lucas Ferreira 👨🏽‍💻", "Thanks! See you tomorrow. 💚"),
        (
            "120363000000003@g.us",
            "Rust community",
            "One app, shared local state.",
        ),
        (
            "12025550119@s.whatsapp.net",
            "Sofia Costa",
            "The photos look amazing.",
        ),
    ] {
        archive
            .name_chat(
                jid.into(),
                name.into(),
                if jid == ANA { 2 } else { 0 },
                base,
            )
            .await?;
        archive
            .store_messages(
                vec![project_message(
                    jid.into(),
                    jid.into(),
                    format!("demo-intro-{jid}"),
                    false,
                    base - 86400,
                    &wa::Message::text(text),
                    String::new(),
                    "{}".into(),
                )],
                false,
            )
            .await?;
    }
    let text_rows = [
        (
            "demo-1",
            ANA,
            false,
            "A little less noise. A little more focus.",
        ),
        (
            "demo-2",
            "self",
            true,
            "Meet Brazap. Native Rust, local history, and a home for your conversations.",
        ),
        ("demo-3", LUCAS, false, "The dark theme feels so good ✨"),
        (
            "demo-4",
            "self",
            true,
            "And your local AI agents can search and reply through MCP.",
        ),
        ("demo-6", "self", true, "Looks great. Let’s ship it!"),
    ];
    for (i, (id, sender, mine, text)) in text_rows.into_iter().enumerate() {
        let mut message = wa::Message::text(text);
        if id == "demo-3" {
            let mut context = wa::ContextInfo::default();
            context.stanza_id = Some("demo-2".into());
            context.participant = Some(OWN.into());
            context.quoted_message = MessageField::some(wa::Message::text(
                "Meet Brazap. Native Rust, local history, and a home for your conversations.",
            ));
            message = wa::Message::text_with_context(text, context);
        }
        let mut row = project_message(
            DESIGN.into(),
            sender.into(),
            id.into(),
            mine,
            base + i as i64 * 60,
            &message,
            String::new(),
            "{}".into(),
        );
        if mine {
            row.view.status = "read".into();
        }
        archive.store_messages(vec![row], false).await?;
    }
    let mut audio = wa::message::AudioMessage::default();
    audio.mimetype = Some("audio/wav".into());
    audio.ptt = Some(true);
    audio.seconds = Some(8);
    audio.waveform = Some((0..64).map(|i| 20 + ((i * 37) % 80) as u8).collect());
    let mut proto = wa::Message::default();
    proto.audio_message = MessageField::some(audio);
    let row = project_message(
        DESIGN.into(),
        ANA.into(),
        "demo-5".into(),
        false,
        base + 210,
        &proto,
        String::new(),
        "{}".into(),
    );
    let message = row.view.clone();
    archive.store_messages(vec![row], false).await?;
    archive.put_media(&message, voice()).await?;

    let bytes = picture()?;
    let mut image = wa::message::ImageMessage::default();
    image.mimetype = Some("image/png".into());
    image.caption = Some("Weekend colors 🌅".into());
    image.width = Some(640);
    image.height = Some(360);
    let mut proto = wa::Message::default();
    proto.image_message = MessageField::some(image);
    let row = project_message(
        "12025550119@s.whatsapp.net".into(),
        "12025550119@s.whatsapp.net".into(),
        "demo-image".into(),
        false,
        base,
        &proto,
        String::new(),
        "{}".into(),
    );
    let message = row.view.clone();
    archive.store_messages(vec![row], false).await?;
    archive.put_media(&message, bytes).await?;
    // Reuse the licensed, bundled emoji artwork as fictional sticker samples.
    let catalog: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../assets/emoji/index.json"))?;
    let art = include_bytes!("../assets/emoji/art.bin");
    for (i, text) in ["💚", "🎉", "✨", "👍", "🦀", "😎"].into_iter().enumerate() {
        let Some(item) = catalog.iter().find(|e| e["text"].as_str() == Some(text)) else {
            continue;
        };
        let offset = item["offset"].as_u64().context("Emoji offset")? as usize;
        let length = item["length"].as_u64().context("Emoji length")? as usize;
        let image = image::load_from_memory(&art[offset..offset + length])?;
        let mut encoded = Cursor::new(Vec::new());
        image.write_to(&mut encoded, image::ImageFormat::WebP)?;
        let bytes = encoded.into_inner();
        let mut sticker = wa::message::StickerMessage::default();
        sticker.mimetype = Some("image/webp".into());
        sticker.file_sha256 = Some(Sha256::digest(&bytes).to_vec());
        sticker.width = Some(image.width());
        sticker.height = Some(image.height());
        sticker.emojis = Some(text.into());
        let mut proto = wa::Message::default();
        proto.sticker_message = MessageField::some(sticker);
        let row = project_message(
            ANA.into(),
            "self".into(),
            format!("demo-sticker-{i}"),
            true,
            base + i as i64,
            &proto,
            String::new(),
            "{}".into(),
        );
        let message = row.view.clone();
        archive.store_messages(vec![row], false).await?;
        archive.put_media(&message, bytes).await?;
    }
    let mut rows = Vec::new();
    for i in 0..140 {
        rows.push(project_message(
            DESIGN.into(),
            ANA.into(),
            format!("demo-history-{i}"),
            false,
            base - 172800 + i * 300,
            &wa::Message::text(format!(
                "Design note {}: keep the conversation clear and the interface simple.",
                i + 1
            )),
            String::new(),
            "{}".into(),
        ));
    }
    archive.store_messages(rows, false).await?;
    for (id, from_me, video, outcome, duration) in [
        (
            "demo-missed-call",
            false,
            false,
            wa::message::call_log_message::CallOutcome::MISSED,
            0,
        ),
        (
            "demo-video-call",
            true,
            true,
            wa::message::call_log_message::CallOutcome::CONNECTED,
            83,
        ),
    ] {
        let mut message = wa::Message::default();
        let mut record = wa::message::CallLogMessage::default();
        record.is_video = Some(video);
        record.call_outcome = Some(outcome);
        record.duration_secs = Some(duration);
        message.call_log_messsage = MessageField::some(record);
        archive
            .store_messages(
                vec![project_message(
                    ANA.into(),
                    if from_me { "self" } else { ANA }.into(),
                    id.into(),
                    from_me,
                    base + if video { 301 } else { 300 },
                    &message,
                    String::new(),
                    "{}".into(),
                )],
                false,
            )
            .await?;
    }
    archive
        .set_chat_action(
            DESIGN.into(),
            crate::chat::ChatAction::Pin,
            true,
            0,
            now() * 1000,
        )
        .await?;
    archive
        .complete_history(DESIGN.into(), "demo_profile")
        .await?;
    archive
        .set_preference("demo_seed_v1".into(), "1".into())
        .await?;
    Ok(())
}

impl Service {
    pub(crate) async fn demo_send(
        &self,
        chat: String,
        proto: wa::Message,
        media: Option<Vec<u8>>,
    ) -> Result<Message> {
        ensure!(self.config.demo, "Local simulation requires demo mode");
        self.archive.allow_chat(&chat).await?;
        let id = format!("demo-{}", uuid::Uuid::new_v4());
        let mut row = outgoing(chat, id, &proto);
        row.view.status = "read".into();
        let mut message = row.view.clone();
        self.archive.store_messages(vec![row], false).await?;
        if let Some(bytes) = media {
            self.archive.put_media(&message, bytes).await?;
            message.has_media = true;
        }
        self.changed();
        Ok(message)
    }

    pub(crate) async fn demo_attachment(
        &self,
        chat: String,
        path: String,
        kind: String,
        caption: String,
    ) -> Result<Message> {
        let path = std::path::Path::new(&path);
        let meta = tokio::fs::metadata(path)
            .await
            .context("Cannot open attachment")?;
        ensure!(
            meta.is_file() && meta.len() <= 64 * 1024 * 1024,
            "Choose a file smaller than 64 MiB"
        );
        let bytes = tokio::fs::read(path).await?;
        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();
        let mut proto = wa::Message::default();
        match kind.as_str() {
            "image" => {
                let mut m = wa::message::ImageMessage::default();
                m.mimetype = Some(mime);
                m.caption = Some(caption);
                proto.image_message = MessageField::some(m);
            }
            "video" => {
                let mut m = wa::message::VideoMessage::default();
                m.mimetype = Some(mime);
                m.caption = Some(caption);
                proto.video_message = MessageField::some(m);
            }
            "audio" => {
                let mut m = wa::message::AudioMessage::default();
                m.mimetype = Some(mime);
                m.ptt = Some(false);
                proto.audio_message = MessageField::some(m);
            }
            "sticker" => {
                ensure!(mime == "image/webp", "Stickers must be WebP files");
                let mut m = wa::message::StickerMessage::default();
                m.mimetype = Some(mime);
                m.file_sha256 = Some(Sha256::digest(&bytes).to_vec());
                proto.sticker_message = MessageField::some(m);
            }
            "file" => {
                let mut m = wa::message::DocumentMessage::default();
                m.mimetype = Some(mime);
                m.caption = Some(caption);
                m.file_name = Some(
                    path.file_name()
                        .context("Missing file name")?
                        .to_string_lossy()
                        .into(),
                );
                proto.document_message = MessageField::some(m);
            }
            _ => anyhow::bail!("Unsupported attachment kind"),
        }
        self.demo_send(chat, proto, Some(bytes)).await
    }
}
