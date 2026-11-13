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

pub(crate) fn pairing() -> crate::model::Connection {
    crate::model::Connection {
        phase: crate::model::Phase::Pairing,
        qr: Some("Brazap offline QR preview - not a WhatsApp pairing code".into()),
        ..Default::default()
    }
}

/// Visual startup fixtures, honored only after the isolated demo profile has
/// been selected. They never create a client or change WhatsApp credentials.
pub(crate) fn connection() -> crate::model::Connection {
    use crate::model::{Connection, Phase};
    match std::env::var("BRAZAP_DEMO_STARTUP").as_deref() {
        Ok("loading") => Connection::default(),
        Ok("qr") => pairing(),
        Ok("connecting") => Connection {
            phase: Phase::Reconnecting,
            has_session: true,
            initial_sync: true,
            ..Default::default()
        },
        Ok("syncing") => Connection {
            phase: Phase::Connected,
            has_session: true,
            initial_sync: true,
            syncing: true,
            sync_progress: Some(45),
            ..Default::default()
        },
        Ok("restoring") => Connection {
            phase: Phase::Reconnecting,
            has_session: true,
            ..Default::default()
        },
        _ => Connection {
            phase: Phase::Connected,
            has_session: true,
            ..Default::default()
        },
    }
}

/// Sidebar update states for isolated visual checks; no network or installer runs.
pub(crate) fn updates() -> crate::updates::UpdateState {
    let status = std::env::var("BRAZAP_DEMO_UPDATE").unwrap_or_default();
    if !matches!(status.as_str(), "ready" | "downloading" | "error") {
        return Default::default();
    }
    crate::updates::UpdateState {
        downloaded: status == "ready",
        status,
        latest: Some("99.0.0".into()),
        ..Default::default()
    }
}

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
    // Offline fixtures for link previews, story progression and queued audio.
    let mut preview = wa::message::ExtendedTextMessage::default();
    preview.text = Some("A useful link: https://www.rust-lang.org/learn".into());
    preview.matched_text = Some("https://www.rust-lang.org/learn".into());
    preview.title = Some("Learn Rust".into());
    preview.description = Some("A collection of resources to get started with Rust.".into());
    preview.jpeg_thumbnail = Some(picture()?);
    let mut link = wa::Message::default();
    link.extended_text_message = MessageField::some(preview);
    archive
        .store_messages(
            vec![project_message(
                DESIGN.into(),
                ANA.into(),
                "demo-link".into(),
                false,
                base + 215,
                &link,
                String::new(),
                "{}".into(),
            )],
            false,
        )
        .await?;
    // Long, unbroken links and long previews must stay inside their bubbles.
    let weekend = "120363000000002@g.us";
    let long_url = "https://www.mercadolivre.com.br/dailus-233-bolo-de-chocolate-esmalte-cremoso-8ml-blz/p/MLB28000507?pdp_filters=item_id%3AMLB5478140251&from=gshop&matt_tool=51266944&matt_word=&matt_source=google&matt_campaign_id=22090354289&matt_ad_group_id=173513864716&matt_match_type=&matt_network=g&matt_device=c&matt_creative=727882725437&matt_keyword=&matt_ad_position=&matt_ad_type=pla&matt_merchant_id=735128188&matt_product_id=MLB28000507-product&matt_product_partition_id=2389869716255&matt_target_id=pla-2389869716255&cq_src=google_ads&cq_cmp=22090354289&cq_net=g&cq_plt=gp&cq_med=pla&gad_source=1&gclid=Cj0KCQiA0MD4BhD7ARIsAKX8YlDemoTrackingValue";
    // WhatsApp falls back to the URL as the title when it cannot read the page.
    let mut bare = wa::message::ExtendedTextMessage::default();
    bare.text = Some(long_url.into());
    bare.matched_text = Some(long_url.into());
    bare.title = Some(long_url.into());
    let mut detailed = wa::message::ExtendedTextMessage::default();
    detailed.text = Some(format!("Found the one we talked about: {long_url}"));
    detailed.matched_text = Some(long_url.into());
    detailed.title = Some(
        "Dailus 233 Bolo de Chocolate Esmalte Cremoso 8ml — Coleção Confeitaria, acabamento cremoso de longa duração, secagem rápida e cobertura uniforme".into(),
    );
    detailed.description = Some(
        "Frete grátis acima de R$ 19. Compre Dailus 233 Bolo de Chocolate Esmalte Cremoso 8ml no Mercado Livre. Encontre mais produtos de Beleza e Cuidado Pessoal, Manicure e Pedicure, Esmaltes. Devolução grátis em 30 dias.".into(),
    );
    detailed.jpeg_thumbnail = Some(picture()?);
    // A page WhatsApp described without an image.
    let mut described = wa::message::ExtendedTextMessage::default();
    let market = "https://www.example.org/events/saturday-farmers-market-and-flower-stalls?utm_source=whatsapp&utm_medium=share";
    described.text = Some(format!("Opening hours are here {market}"));
    described.matched_text = Some(market.into());
    described.title = Some("Saturday Farmers Market — opening hours, stalls and how to get there by bus, bike or on foot".into());
    described.description = Some("Fresh produce, flowers and local bakers every Saturday from 8 am to 1 pm. Check the map for the new stall layout and accessible entrances before you go.".into());
    let long_text = "Planning notes for Saturday, so nobody has to scroll back through the chat: we meet at the bakery at nine, then walk to the market before it gets crowded. Bring cash for the flower stall, a reusable bag, and sunscreen because the forecast says it will be sunny all day.\n\nIf anyone is running late, send a message here and we will save you a seat. After lunch we can decide between the museum and the park depending on the weather.";
    let mut rows = Vec::new();
    for (offset, id, sender, proto) in [
        (0, "demo-long-link", LUCAS, {
            let mut proto = wa::Message::default();
            proto.extended_text_message = MessageField::some(bare);
            proto
        }),
        (60, "demo-long-preview", ANA, {
            let mut proto = wa::Message::default();
            proto.extended_text_message = MessageField::some(detailed);
            proto
        }),
        (90, "demo-described-link", LUCAS, {
            let mut proto = wa::Message::default();
            proto.extended_text_message = MessageField::some(described);
            proto
        }),
        (120, "demo-long-text", "self", wa::Message::text(long_text)),
    ] {
        let mut row = project_message(
            weekend.into(),
            sender.into(),
            id.into(),
            sender == "self",
            base - 90000 + offset,
            &proto,
            String::new(),
            "{}".into(),
        );
        if sender == "self" {
            row.view.status = "read".into();
        }
        rows.push(row);
    }
    archive.store_messages(rows, false).await?;
    for index in 0..2 {
        let mut audio = wa::message::AudioMessage::default();
        audio.mimetype = Some("audio/wav".into());
        audio.seconds = Some(3);
        let mut proto = wa::Message::default();
        proto.audio_message = MessageField::some(audio);
        let row = project_message(
            DESIGN.into(),
            ANA.into(),
            format!("demo-audio-next-{index}"),
            false,
            base + 212 + index,
            &proto,
            String::new(),
            "{}".into(),
        );
        let message = row.view.clone();
        archive.store_messages(vec![row], false).await?;
        archive.put_media(&message, voice()).await?;
    }
    for (index, sender, text, color) in [
        (0, ANA, "A little sunshine for your day ☀️", 0xff184a47),
        (1, ANA, "Ready for the weekend! 🌿", 0xff374971),
        (2, LUCAS, "Working on something new ✨", 0xff51325c),
    ] {
        let mut body = wa::message::ExtendedTextMessage::default();
        body.text = Some(text.into());
        body.background_argb = Some(color);
        let mut proto = wa::Message::default();
        proto.extended_text_message = MessageField::some(body);
        archive
            .store_messages(
                vec![project_message(
                    crate::stories::CHAT.into(),
                    sender.into(),
                    format!("demo-status-{index}"),
                    false,
                    base + 600 + index,
                    &proto,
                    String::new(),
                    "{}".into(),
                )],
                false,
            )
            .await?;
    }
    let mut image = wa::message::ImageMessage::default();
    image.mimetype = Some("image/png".into());
    image.caption = Some("Weekend colors 🌅".into());
    let mut proto = wa::Message::default();
    proto.image_message = MessageField::some(image);
    let row = project_message(
        crate::stories::CHAT.into(),
        LUCAS.into(),
        "demo-status-image".into(),
        false,
        base + 605,
        &proto,
        String::new(),
        "{}".into(),
    );
    let message = row.view.clone();
    archive.store_messages(vec![row], false).await?;
    archive.put_media(&message, picture()?).await?;
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
