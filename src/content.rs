use crate::model::{Message, MessageAction};
use std::borrow::Cow;
use whatsapp_rust::prelude::{MessageExt, wa};

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

/// The archive keeps the original protobuf. This is only its user-visible projection.
pub fn project(view: &mut Message, message: &wa::Message) {
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
    if let Some(context) = base.context_info() {
        view.forwarded = context.is_forwarded.unwrap_or(false);
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
            let text = quoted
                .text_content()
                .or_else(|| quoted.get_caption())
                .unwrap_or_else(|| {
                    if quoted.image_message.is_set() {
                        "Photo"
                    } else if quoted.video_message.is_set() {
                        "Video"
                    } else if quoted.sticker_message.is_set() {
                        "Sticker"
                    } else if quoted.audio_message.is_set() {
                        "Voice message"
                    } else if quoted.document_message.is_set() {
                        "Document"
                    } else {
                        "Message"
                    }
                });
            view.quote = Some(crate::model::Quote {
                chat: context.remote_jid.clone(),
                id: id.clone(),
                sender: context.participant.clone().unwrap_or_default(),
                text: text.chars().take(280).collect(),
                ..Default::default()
            });
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
        view.text = t.display_name.clone().unwrap_or_else(|| "Contact".into());
    } else if let Some(t) = base.contacts_array_message.as_option() {
        view.kind = "contact".into();
        view.text = t
            .contacts
            .iter()
            .filter_map(|c| c.display_name.as_deref())
            .collect::<Vec<_>>()
            .join(", ");
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
    let media = media_message(message);
    if let Some(m) = media.image_message.as_option() {
        view.kind = "image".into();
        view.mime = m.mimetype.clone();
    } else if let Some(m) = media.video_message.as_option() {
        view.kind = "video".into();
        view.mime = m.mimetype.clone();
        view.duration_seconds = m.seconds;
    } else if let Some(m) = media.audio_message.as_option() {
        view.kind = "audio".into();
        view.mime = m.mimetype.clone();
        view.duration_seconds = m.seconds;
        view.waveform = m.waveform.clone().unwrap_or_default();
    } else if let Some(m) = media.sticker_message.as_option() {
        view.kind = "sticker".into();
        view.mime = m.mimetype.clone();
    } else if let Some(m) = media.document_message.as_option() {
        view.kind = "file".into();
        view.mime = m.mimetype.clone();
        view.file_name = m.file_name.clone().or_else(|| m.title.clone());
    }
    if view.kind == "text" && view.text.trim().is_empty() && view.title.is_empty() {
        view.kind = "unsupported".into();
        view.text = "This message type is not supported yet.".into();
    }
}

/// Expose media from business-message headers through the same download/cache path.
pub fn media_message(message: &wa::Message) -> Cow<'_, wa::Message> {
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
