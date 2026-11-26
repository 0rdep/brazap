use crate::model::{Message, MessageAction};
use std::borrow::Cow;
use whatsapp_rust::{
    prelude::{MessageExt, wa},
    waproto::buffa::MessageField,
};

fn call(view: &mut Message, video: Option<bool>, outcome: Option<&str>, duration: Option<u32>) {
    let (kind, label) = match video {
        Some(true) => ("video_call", "Video call"),
        Some(false) => ("voice_call", "Voice call"),
        None => ("call", "Call"),
    };
    view.kind = kind.into();
    view.text = label.into();
    view.duration_seconds = duration;
    view.footer = outcome.unwrap_or_default().into();
}

pub(crate) fn project_call(view: &mut Message, message: &wa::Message) -> bool {
    use wa::message::{b_call_message::MediaType, call_log_message::CallOutcome};
    let base = message.get_base_message();
    // The extra 's' is the field spelling in WhatsApp's protobuf schema.
    if let Some(record) = base.call_log_messsage.as_option() {
        let outcome = match record.call_outcome {
            Some(CallOutcome::MISSED) if view.from_me => Some("Unanswered"),
            Some(CallOutcome::MISSED) => Some("Missed"),
            Some(CallOutcome::FAILED) => Some("Failed"),
            Some(CallOutcome::REJECTED) => Some("Declined"),
            Some(CallOutcome::ACCEPTED_ELSEWHERE) => Some("Answered on another device"),
            Some(CallOutcome::ONGOING) => Some("Ongoing"),
            Some(CallOutcome::SILENCED_BY_DND | CallOutcome::SILENCED_UNKNOWN_CALLER) => {
                Some("Silenced")
            }
            _ => None,
        };
        call(
            view,
            record.is_video,
            outcome,
            record.duration_secs.and_then(|n| u32::try_from(n).ok()),
        );
    } else if let Some(record) = base.bcall_message.as_option() {
        let video = match record.media_type {
            Some(MediaType::VIDEO) => Some(true),
            Some(MediaType::AUDIO) => Some(false),
            _ => None,
        };
        call(view, video, None, None);
    } else if base.call.is_set() {
        call(view, None, None, None);
    } else {
        return false;
    }
    true
}

pub(crate) fn project_call_stub(view: &mut Message, metadata: &str) -> bool {
    if !metadata.contains("messageStubType") && !metadata.contains("message_stub_type") {
        return false;
    }
    let Ok(info) = serde_json::from_str::<serde_json::Value>(metadata) else {
        return false;
    };
    let Some(stub) = info
        .get("messageStubType")
        .or_else(|| info.get("message_stub_type"))
    else {
        return false;
    };
    let video = match (stub.as_str(), stub.as_i64()) {
        (Some("CALL_MISSED_VOICE" | "CALL_MISSED_GROUP_VOICE"), _) | (_, Some(40 | 45)) => false,
        (Some("CALL_MISSED_VIDEO" | "CALL_MISSED_GROUP_VIDEO"), _) | (_, Some(41 | 46)) => true,
        _ => return false,
    };
    call(
        view,
        Some(video),
        Some(if view.from_me { "Unanswered" } else { "Missed" }),
        None,
    );
    true
}

fn action(view: &mut Message, label: Option<&str>, kind: &str, target: Option<&str>) {
    if let Some(label) = label.filter(|s| !s.trim().is_empty()) {
        view.actions.push(MessageAction {
            label: label.to_owned(),
            target: target.filter(|s| !s.is_empty()).map(str::to_owned),
            kind: kind.into(),
            ..Default::default()
        });
    }
}

fn reply(
    view: &mut Message,
    label: Option<&str>,
    flow: &str,
    id: Option<&str>,
    index: Option<u32>,
) {
    let before = view.actions.len();
    action(view, label, "reply", None);
    if let Some(button) = view.actions.get_mut(before) {
        button.flow = flow.into();
        button.id = id.map(str::to_owned);
        button.index = index.unwrap_or(before as u32);
    }
}

/// The Pix "copy and paste" code of a payment button, when the business included one.
fn pix_code(params: &serde_json::Value) -> Option<&str> {
    params
        .get("copy_code")
        .and_then(|v| v.as_str())
        .or_else(|| {
            params
                .get("payment_settings")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .chain(params.get("payment_setting"))
                .find_map(|setting| {
                    setting
                        .get("pix_dynamic_code")
                        .and_then(|v| v.get("code"))
                        .and_then(|v| v.as_str())
                })
        })
        .filter(|code| !code.trim().is_empty())
}

/// Buttons of an interactive ("native flow") message. `name` identifies the button type and
/// `params` carries its details as JSON.
fn native_flow(view: &mut Message, name: &str, params: &str) {
    let params = serde_json::from_str::<serde_json::Value>(params).unwrap_or_default();
    let text = |key: &str| {
        params
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
    };
    let label = text("display_text");
    match name {
        "quick_reply" => reply(view, label, "native", text("id"), None),
        "cta_url" => action(view, label, "url", text("url").or(text("merchant_url"))),
        "cta_call" => action(view, label, "call", text("phone_number")),
        "voice_call" => action(view, label.or(Some("Call")), "voice_call", None),
        "cta_copy" => action(view, label, "copy", text("copy_code")),
        _ => {
            if let Some(code) = pix_code(&params) {
                action(view, label.or(Some("Copy Pix code")), "copy", Some(code));
            } else if let Some(url) = text("url") {
                action(view, label, "url", Some(url));
            } else if let Some(code) = text("copy_code") {
                action(view, label, "copy", Some(code));
            } else if let Some(phone) = text("phone_number") {
                action(view, label, "call", Some(phone));
            } else {
                action(view, label, "unsupported", None);
            }
        }
    }
}

fn hydrated(view: &mut Message, t: &wa::message::template_message::HydratedFourRowTemplate) {
    use wa::message::template_message::hydrated_four_row_template::Title;
    view.kind = "template".into();
    view.text = t.hydrated_content_text.clone().unwrap_or_default();
    view.footer = t.hydrated_footer_text.clone().unwrap_or_default();
    if let Some(Title::HydratedTitleText(text)) = &t.title {
        view.title = text.clone();
    }
    for button in &t.hydrated_buttons {
        use wa::hydrated_template_button::HydratedButton;
        match &button.hydrated_button {
            Some(HydratedButton::QuickReplyButton(b)) => reply(
                view,
                b.display_text.as_deref(),
                "template",
                b.id.as_deref(),
                button.index,
            ),
            Some(HydratedButton::UrlButton(b)) => {
                action(view, b.display_text.as_deref(), "url", b.url.as_deref())
            }
            Some(HydratedButton::CallButton(b)) => action(
                view,
                b.display_text.as_deref(),
                "call",
                b.phone_number.as_deref(),
            ),
            _ => {}
        }
    }
}

fn interactive(view: &mut Message, t: &wa::message::InteractiveMessage) {
    view.kind = "template".into();
    if let Some(header) = t.header.as_option() {
        view.title = header.title.clone().unwrap_or_default();
    }
    view.text = t
        .body
        .as_option()
        .and_then(|body| body.text.clone())
        .unwrap_or_default();
    view.footer = t
        .footer
        .as_option()
        .and_then(|footer| footer.text.clone())
        .unwrap_or_default();
    if let Some(wa::message::interactive_message::InteractiveMessage::NativeFlowMessage(flow)) =
        &t.interactive_message
    {
        for button in &flow.buttons {
            native_flow(
                view,
                button.name.as_deref().unwrap_or_default(),
                button.button_params_json.as_deref().unwrap_or("{}"),
            );
        }
    }
}

fn template(view: &mut Message, t: &wa::message::TemplateMessage) {
    use wa::message::template_message::Format;
    if let Some(h) = t.hydrated_template.as_option() {
        hydrated(view, h);
        return;
    }
    match &t.format {
        Some(Format::HydratedFourRowTemplate(h)) => hydrated(view, h),
        Some(Format::InteractiveMessageTemplate(t)) => interactive(view, t),
        Some(Format::FourRowTemplate(t)) => {
            view.kind = "template".into();
            if let Some(hsm) = t.content.as_option() {
                view.text = hsm.params.join("\n");
            }
            if let Some(hsm) = t.footer.as_option() {
                view.footer = hsm.params.join("\n");
            }
            if view.text.is_empty() {
                view.text =
                    "Business message — the sender did not include its translated text.".into();
            }
        }
        _ => {
            view.kind = "template".into();
            view.text = "Business message — content unavailable on this device.".into();
        }
    }
}

/// Tags group members (and everyone, as `@all` does) in a photo, video or
/// document caption, the way a text message tags them.
pub fn tag_caption(message: &mut wa::Message, mentions: Vec<String>, everyone: bool) {
    if mentions.is_empty() && !everyone {
        return;
    }
    let context = if let Some(m) = message.image_message.as_option_mut() {
        &mut m.context_info
    } else if let Some(m) = message.video_message.as_option_mut() {
        &mut m.context_info
    } else if let Some(m) = message.document_message.as_option_mut() {
        &mut m.context_info
    } else {
        return;
    };
    let mut info = context.as_option().cloned().unwrap_or_default();
    info.mentioned_jid = mentions;
    if everyone {
        info.non_jid_mentions = Some(1);
    }
    *context = whatsapp_rust::waproto::buffa::MessageField::some(info);
}

/// A sticker with sound arrives wrapped (`audioStickerMessage`), carrying its audio
/// inside the sticker. Read it as the sticker it contains; the archive keeps the
/// original protobuf.
pub fn unwrap_audio_sticker(message: &wa::Message) -> Cow<'_, wa::Message> {
    match message
        .get_base_message()
        .audio_sticker_message
        .as_option()
        .and_then(|wrapper| wrapper.message.as_option())
    {
        Some(inner) => Cow::Owned(inner.clone()),
        None => Cow::Borrowed(message),
    }
}

/// The audio a sticker with sound plays, when it has one.
pub fn sticker_audio(message: &wa::Message) -> Option<wa::message::AudioMessage> {
    use whatsapp_rust::waproto::whatsapp::__buffa::oneof::message::sticker_message::Audio;
    let message = unwrap_audio_sticker(message);
    let base = message.get_base_message();
    match &base.sticker_message.as_option()?.audio {
        Some(Audio::AudioMessage(audio)) => Some((**audio).clone()),
        _ => None,
    }
}

/// The archive keeps the original protobuf. This is only its user-visible projection.
pub fn project(view: &mut Message, message: &wa::Message) {
    if let Some(audio) = sticker_audio(message) {
        view.sticker_sound = true;
        view.duration_seconds = audio.seconds;
    }
    let message = unwrap_audio_sticker(message);
    let message = message.as_ref();
    let base = message.get_base_message();
    if let Some(text) = base.extended_text_message.as_option() {
        view.background_argb = text.background_argb;
        if let Some((_, url)) = crate::links::detect(
            text.matched_text
                .as_deref()
                .filter(|text| !text.trim().is_empty())
                .unwrap_or(&view.text),
        )
        .into_iter()
        .next()
        {
            view.link_preview = Some(crate::model::LinkPreview {
                url,
                title: text.title.clone().unwrap_or_default(),
                description: text.description.clone().unwrap_or_default(),
            });
        }
    }
    if view.link_preview.is_none()
        && let Some((_, url)) = crate::links::detect(&view.text).into_iter().next()
    {
        view.link_preview = Some(crate::model::LinkPreview {
            url,
            ..Default::default()
        });
    }
    if project_call(view, message) {
        return;
    }
    if let Some(association) = message
        .message_context_info
        .as_option()
        .and_then(|info| info.message_association.as_option())
        .filter(|a| {
            a.association_type == Some(wa::message_association::AssociationType::MEDIA_ALBUM)
        })
    {
        view.album = association
            .parent_message_key
            .as_option()
            .and_then(|key| key.id.clone());
    }
    if let Some(context) = base.context_info() {
        view.forwarded = context.is_forwarded.unwrap_or(false);
        // Bit 1 of nonJidMentions: `@all` tags everyone in the group.
        view.mentions_everyone = context.non_jid_mentions.is_some_and(|bits| bits & 1 != 0);
        for jid in &context.mentioned_jid {
            if !jid.is_empty() && !view.mentions.iter().any(|m| &m.jid == jid) {
                view.mentions.push(crate::model::Mention {
                    jid: jid.clone(),
                    ..Default::default()
                });
            }
        }
        if let (Some(id), Some(quoted)) = (&context.stanza_id, context.quoted_message.as_option()) {
            let quoted = quoted.get_base_message();
            let kind = if quoted.image_message.is_set() {
                "image"
            } else if quoted.video_message.is_set() {
                "video"
            } else if quoted.sticker_message.is_set() {
                "sticker"
            } else if quoted.audio_message.is_set() {
                "audio"
            } else if quoted.document_message.is_set() {
                "file"
            } else {
                ""
            };
            let mut quote = crate::model::Quote {
                chat: context.remote_jid.clone(),
                id: id.clone(),
                sender: context.participant.clone().unwrap_or_default(),
                kind: kind.into(),
                ..Default::default()
            };
            // A photo sent without a caption has an empty one, not none.
            let text = quoted
                .text_content()
                .or_else(|| quoted.get_caption())
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| quote.fallback());
            quote.text = text.chars().take(280).collect();
            view.quote = Some(quote);
        }
    }
    if let Some(reaction) = base.reaction_message.as_option() {
        if let Some(key) = reaction.key.as_option() {
            view.reaction_update = Some(crate::model::ReactionUpdate {
                target_sender: if key.from_me.unwrap_or(false) {
                    "self".into()
                } else {
                    key.participant.clone().unwrap_or_else(|| view.chat.clone())
                },
                target_id: key.id.clone().unwrap_or_default(),
                emoji: reaction.text.clone().unwrap_or_default(),
                timestamp_ms: reaction
                    .sender_timestamp_ms
                    .unwrap_or(view.timestamp.saturating_mul(1000)),
            });
        }
        view.kind = "protocol".into();
        return;
    }
    if crate::edits::project(view, base) {
        return;
    }
    if let Some(t) = base.template_message.as_option() {
        template(view, t);
    } else if let Some(hsm) = base.highly_structured_message.as_option() {
        if let Some(t) = hsm.hydrated_hsm.as_option() {
            template(view, t);
        } else {
            view.kind = "template".into();
            view.text = if hsm.params.is_empty() {
                "Business message — content unavailable on this device.".into()
            } else {
                hsm.params.join("\n")
            };
        }
    } else if let Some(t) = base.interactive_message.as_option() {
        interactive(view, t);
    } else if let Some(t) = base.buttons_message.as_option() {
        view.kind = "template".into();
        view.text = t.content_text.clone().unwrap_or_default();
        view.footer = t.footer_text.clone().unwrap_or_default();
        if let Some(wa::message::buttons_message::Header::Text(text)) = &t.header {
            view.title = text.clone();
        }
        for b in &t.buttons {
            let label = b
                .button_text
                .as_option()
                .and_then(|b| b.display_text.as_deref());
            if let Some(flow) = b.native_flow_info.as_option() {
                native_flow(
                    view,
                    flow.name.as_deref().unwrap_or_default(),
                    flow.params_json.as_deref().unwrap_or("{}"),
                );
            } else {
                reply(view, label, "buttons", b.button_id.as_deref(), None);
            }
        }
    } else if let Some(t) = base.template_button_reply_message.as_option() {
        view.text = t.selected_display_text.clone().unwrap_or_default();
    } else if let Some(t) = base.buttons_response_message.as_option() {
        if let Some(wa::message::buttons_response_message::Response::SelectedDisplayText(text)) =
            &t.response
        {
            view.text = text.clone();
        }
    } else if let Some(t) = base.interactive_response_message.as_option() {
        view.text = t
            .body
            .as_option()
            .and_then(|b| b.text.clone())
            .unwrap_or_default();
    } else if let Some(t) = base.list_message.as_option() {
        view.kind = "template".into();
        view.title = t.title.clone().unwrap_or_default();
        view.text = t.description.clone().unwrap_or_default();
        view.footer = t.footer_text.clone().unwrap_or_default();
        for section in &t.sections {
            for row in &section.rows {
                let at = view.actions.len();
                reply(
                    view,
                    row.title.as_deref(),
                    "list",
                    row.row_id.as_deref(),
                    None,
                );
                if let Some(button) = view.actions.get_mut(at) {
                    button.target = row.description.clone().filter(|d| !d.trim().is_empty());
                }
            }
        }
    } else if let Some(t) = base.list_response_message.as_option() {
        view.text = t.title.clone().unwrap_or_default();
    } else if let Some(t) = base.contact_message.as_option() {
        view.kind = "contact".into();
        let card = shared_contact(t);
        view.text = card.name.clone();
        view.contacts = vec![card];
    } else if let Some(t) = base.contacts_array_message.as_option() {
        view.kind = "contact".into();
        view.contacts = t.contacts.iter().map(shared_contact).collect();
        view.text = view
            .contacts
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
    } else if let Some(t) = base.pin_in_chat_message.as_option() {
        // "Pinned a message": a note in the conversation that leads to the pinned one.
        view.kind = "pin".into();
        let unpin = t.r#type == Some(wa::message::pin_in_chat_message::Type::UNPIN_FOR_ALL);
        view.text = if unpin {
            "Unpinned a message"
        } else {
            "Pinned a message"
        }
        .into();
        if let Some(key) = t.key.as_option() {
            // The key is the pinner's view: "from me" is the pinner's own message.
            let sender = if key.from_me.unwrap_or(false) {
                view.sender.clone()
            } else {
                key.participant.clone().unwrap_or_else(|| view.chat.clone())
            };
            view.quote = Some(crate::model::Quote {
                chat: key.remote_jid.clone(),
                id: key.id.clone().unwrap_or_default(),
                sender,
                ..Default::default()
            });
        }
    } else if let Some(t) = base.location_message.as_option() {
        view.kind = "location".into();
        view.text = t
            .name
            .clone()
            .or_else(|| t.address.clone())
            .unwrap_or_else(|| "Shared location".into());
    } else if base.protocol_message.is_set()
        || base.sender_key_distribution_message.is_set()
        || base
            .fast_ratchet_key_sender_key_distribution_message
            .is_set()
    {
        view.kind = "protocol".into();
    } else if base.placeholder_message.is_set() {
        view.kind = "placeholder".into();
        view.text = "Message unavailable on this linked device".into();
    }
    if project_view_once(view, message) {
        return;
    }
    let media = media_message(message);
    if let Some(m) = media.image_message.as_option() {
        view.kind = "image".into();
        view.mime = m.mimetype.clone();
    } else if let Some(m) = media.video_message.as_option() {
        view.kind = "video".into();
        view.mime = m.mimetype.clone();
        view.duration_seconds = m.seconds;
        view.gif = m.gif_playback == Some(true);
    } else if let Some(m) = media.audio_message.as_option() {
        view.kind = "audio".into();
        view.mime = m.mimetype.clone();
        view.duration_seconds = m.seconds;
        view.waveform = m.waveform.clone().unwrap_or_default();
    } else if let Some(m) = media.sticker_message.as_option() {
        view.kind = "sticker".into();
        view.mime = m.mimetype.clone();
        view.sticker_hash = m.file_sha256.as_ref().map(|hash| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(hash)
        });
    } else if let Some(m) = media.document_message.as_option() {
        view.kind = "file".into();
        view.mime = m.mimetype.clone();
        view.file_name = m.file_name.clone().or_else(|| m.title.clone());
    }
    if base.album_message.is_set() {
        // Its photos and videos arrive as their own messages that point back here.
        view.kind = "album".into();
        return;
    }
    if view.kind == "text" && view.text.trim().is_empty() && view.title.is_empty() {
        view.kind = "unsupported".into();
        view.text = "This message type is not supported yet.".into();
    }
}

/// A shared contact card: its name, business name and numbers from the vCard.
fn shared_contact(message: &wa::message::ContactMessage) -> crate::model::SharedContact {
    let mut card = crate::model::SharedContact::default();
    let vcard = message.vcard.as_deref().unwrap_or_default();
    // Folded lines continue with a space or tab.
    let mut lines: Vec<String> = Vec::new();
    for line in vcard.lines() {
        match (line.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ => lines.push(line.trim_end_matches('\r').to_owned()),
        }
    }
    let mut full_name = String::new();
    for line in &lines {
        let Some((property, value)) = line.split_once(':') else {
            continue;
        };
        let mut params = property.split(';');
        // Apple's "item1.TEL" groups a property with its label.
        let name = params.next().unwrap_or_default();
        let name = name.rsplit('.').next().unwrap_or(name).to_ascii_uppercase();
        let value = value
            .replace("\\n", " ")
            .replace("\\,", ",")
            .replace("\\;", ";")
            .trim()
            .to_owned();
        match name.as_str() {
            "FN" => full_name = value,
            "X-WA-BIZ-NAME" => card.business = value,
            "TEL" => {
                let waid = params
                    .filter_map(|p| p.split_once('='))
                    .find(|(key, _)| key.eq_ignore_ascii_case("waid"))
                    .map(|(_, id)| id.trim().to_owned())
                    .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
                    .unwrap_or_default();
                if !value.is_empty() || !waid.is_empty() {
                    card.phones.push(crate::model::SharedPhone {
                        number: if value.is_empty() {
                            format!("+{waid}")
                        } else {
                            value
                        },
                        waid,
                    });
                }
            }
            _ => {}
        }
    }
    card.name = message
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .or_else(|| (!full_name.is_empty()).then_some(full_name))
        .or_else(|| (!card.business.is_empty()).then(|| card.business.clone()))
        .or_else(|| card.phones.first().map(|phone| phone.number.clone()))
        .unwrap_or_else(|| "Contact".into());
    card
}

/// The error for any attempt to open, save or forward view-once media.
pub(crate) const VIEW_ONCE_LOCKED: &str = "View once media can only be opened on your phone";

/// View-once photos, videos and voice messages. The caption is part of the hidden
/// content, so only the media type is shown. An empty wrapper is the placeholder
/// stored for WhatsApp's view-once `<unavailable>` fanout to linked devices.
/// Only `encoded` keeps the media keys; nothing projects or downloads them.
fn project_view_once(view: &mut Message, message: &wa::Message) -> bool {
    if !message.is_view_once() {
        return false;
    }
    let base = message.get_base_message();
    let (kind, mime, seconds) = if let Some(m) = base.image_message.as_option() {
        ("image", m.mimetype.clone(), None)
    } else if let Some(m) = base.video_message.as_option() {
        ("video", m.mimetype.clone(), m.seconds)
    } else if let Some(m) = base.ptv_message.as_option() {
        ("video", m.mimetype.clone(), m.seconds)
    } else if let Some(m) = base.audio_message.as_option() {
        ("audio", m.mimetype.clone(), m.seconds)
    } else {
        // An empty wrapper, or content this build cannot tell apart.
        ("", None, None)
    };
    view.kind = "view_once".into();
    view.view_once = kind.into();
    view.mime = mime;
    view.duration_seconds = seconds;
    view.text.clear();
    view.title.clear();
    view.footer.clear();
    view.mentions.clear();
    view.link_preview = None;
    view.background_argb = None;
    true
}

/// Wrap a photo, video or voice message the way WhatsApp's apps send view-once
/// media: the inline `view_once` flag inside `viewOnceMessageV2`, or
/// `viewOnceMessageV2Extension` for voice messages.
pub(crate) fn into_view_once(mut message: wa::Message) -> anyhow::Result<wa::Message> {
    let voice = if let Some(m) = message.image_message.as_option_mut() {
        m.view_once = Some(true);
        false
    } else if let Some(m) = message.video_message.as_option_mut() {
        m.view_once = Some(true);
        false
    } else if let Some(m) = message
        .audio_message
        .as_option_mut()
        .filter(|m| m.ptt == Some(true))
    {
        m.view_once = Some(true);
        true
    } else {
        anyhow::bail!("{VIEW_ONCE_KINDS}");
    };
    let mut wrapper = wa::message::FutureProofMessage::default();
    wrapper.message = MessageField::some(message);
    let mut wrapped = wa::Message::default();
    if voice {
        wrapped.view_once_message_v2_extension = MessageField::some(wrapper);
    } else {
        wrapped.view_once_message_v2 = MessageField::some(wrapper);
    }
    Ok(wrapped)
}

pub(crate) const VIEW_ONCE_KINDS: &str =
    "Only photos, videos and voice messages can be sent as view once";

/// The stand-in kept for a view-once message WhatsApp did not deliver to this
/// linked device: an empty view-once wrapper, which projects as its placeholder.
pub(crate) fn view_once_placeholder() -> wa::Message {
    let mut wrapper = wa::message::FutureProofMessage::default();
    wrapper.message = MessageField::some(wa::Message::default());
    let mut message = wa::Message::default();
    message.view_once_message_v2 = MessageField::some(wrapper);
    message
}

/// A copy of a message to quote in a reply. A quoted view-once message keeps its
/// type but not the keys and preview that would let the recipient open it again.
pub(crate) fn quotable(message: &wa::Message) -> Cow<'_, wa::Message> {
    if !message.is_view_once() {
        return Cow::Borrowed(message);
    }
    fn strip(message: &mut wa::Message) {
        macro_rules! clear {
            ($field:ident) => {
                if let Some(m) = message.$field.as_option_mut() {
                    m.url = None;
                    m.direct_path = None;
                    m.media_key = None;
                    m.file_enc_sha256 = None;
                    m.jpeg_thumbnail = None;
                }
            };
        }
        clear!(image_message);
        clear!(video_message);
        clear!(ptv_message);
        if let Some(m) = message.audio_message.as_option_mut() {
            m.url = None;
            m.direct_path = None;
            m.media_key = None;
            m.file_enc_sha256 = None;
        }
        for inner in [
            message
                .device_sent_message
                .as_option_mut()
                .map(|m| &mut m.message),
            message
                .ephemeral_message
                .as_option_mut()
                .map(|m| &mut m.message),
            message
                .view_once_message
                .as_option_mut()
                .map(|m| &mut m.message),
            message
                .view_once_message_v2
                .as_option_mut()
                .map(|m| &mut m.message),
            message
                .view_once_message_v2_extension
                .as_option_mut()
                .map(|m| &mut m.message),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(inner) = inner.as_option_mut() {
                strip(inner);
            }
        }
    }
    let mut copy = message.clone();
    strip(&mut copy);
    Cow::Owned(copy)
}

/// Expose media from business-message headers through the same download/cache path.
pub fn media_message(message: &wa::Message) -> Cow<'_, wa::Message> {
    if let Cow::Owned(inner) = unwrap_audio_sticker(message) {
        let mut sticker = wa::Message::default();
        sticker.sticker_message = inner.get_base_message().sticker_message.clone();
        if sticker.sticker_message.is_set() {
            return Cow::Owned(sticker);
        }
    }
    let base = message.get_base_message();
    macro_rules! wrap {
        ($field:ident,$value:expr) => {{
            let mut m = wa::Message::default();
            m.$field = whatsapp_rust::waproto::buffa::MessageField::some((**$value).clone());
            return Cow::Owned(m);
        }};
    }
    macro_rules! title_media {
        ($title:expr,$ty:path) => {{
            use $ty as Title;
            match $title {
                Some(Title::ImageMessage(v)) => wrap!(image_message, v),
                Some(Title::VideoMessage(v)) => wrap!(video_message, v),
                Some(Title::DocumentMessage(v)) => wrap!(document_message, v),
                _ => {}
            }
        }};
    }
    if let Some(buttons) = base.buttons_message.as_option() {
        title_media!(&buttons.header, wa::message::buttons_message::Header);
    }
    let template = base.template_message.as_option().or_else(|| {
        base.highly_structured_message
            .as_option()
            .and_then(|h| h.hydrated_hsm.as_option())
    });
    if let Some(t) = template {
        if let Some(h) = t.hydrated_template.as_option() {
            title_media!(
                &h.title,
                wa::message::template_message::hydrated_four_row_template::Title
            );
        }
        use wa::message::template_message::Format;
        match &t.format {
            Some(Format::HydratedFourRowTemplate(h)) => title_media!(
                &h.title,
                wa::message::template_message::hydrated_four_row_template::Title
            ),
            Some(Format::FourRowTemplate(h)) => title_media!(
                &h.title,
                wa::message::template_message::four_row_template::Title
            ),
            _ => {}
        }
    }
    let interactive = base.interactive_message.as_option().or_else(|| {
        template.and_then(|t| {
            if let Some(wa::message::template_message::Format::InteractiveMessageTemplate(t)) =
                &t.format
            {
                Some(t.as_ref())
            } else {
                None
            }
        })
    });
    if let Some(t) = interactive.and_then(|t| t.header.as_option()) {
        title_media!(&t.media, wa::message::interactive_message::header::Media);
    }
    Cow::Borrowed(base)
}
