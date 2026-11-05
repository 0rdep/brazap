# Voice and video calls

Brazap's first live-call integration supports one-to-one WhatsApp calls. Use the
phone or camera button in a private chat's header. An incoming call offers
**Answer**, or **Answer with audio** / **Answer with video** for a video call.
You can mute your microphone, toggle the camera, minimize the call, or hang up.
Minimizing the call panel opens a separate, always-on-top native window for audio
or video. Drag its header anywhere on the desktop while continuing to use chats.
Its controls share the same call; the return button (or Escape) brings the full
call back to Brazap. Closing the floating window does not hang up. The window is
created on demand and video pixels are shared with the main window.

The default system microphone, speaker, and camera are used. Allow camera and
microphone access in your operating system. A headset is recommended: this
initial device bridge does not include acoustic echo cancellation. A failed
camera, denied permission, or a disconnected camera switches to a synthetic black
video feed. You can still send/answer video calls and see the other person without
a webcam. The call panel identifies black video; turn video off and on to retry
the camera. Codec or audio-device failures still report an error.

Call state, duration and outcome are saved in the shared SQLite archive. An
unfinished lifecycle journal is recovered as an interrupted call on restart.
Call audio/video are transient and are not recorded. Device workers and their
bounded queues stop when the call ends; codecs are initialized on demand.

Group calls, device selection, screen sharing, and advanced audio processing are
not implemented in this first integration. Incoming group calls are left to your
other linked devices. Meta currently supports user-initiated calls to Cloud API
businesses only from primary or companion iPhone/Android phones, excluding desktop
callers; see [Meta's device eligibility documentation](https://developers.facebook.com/documentation/business-messaging/whatsapp/calling/user-initiated-calls).
Native builds and offline previews do not establish
real-device interoperability; validate an incoming and outgoing call with a
second WhatsApp device before relying on it.

## Local agents

`get_current_call`, `start_call`, `answer_call`, `end_call`, `mute_call`, and
`camera_call` use the same call/session as the desktop. Starting or answering a
call can activate this computer's microphone and camera and contact a real
person. Use exact private-chat identifiers from `list_chats`.

## Offline development

Run `brazap --demo --data-dir <empty-directory>`. Starting a call uses fictional
contacts and no WhatsApp client or physical microphone/camera. Video runs the
same synthetic black source through the bundled H.264 encoder and decoder. The internal
`simulate_incoming_call` operation is only available for a demo profile and routes
through the normal incoming-offer handler. Calls from known, unlocked chats can
ring even when the global chat-privacy refresh fails; locked chats, unverified
chat visibility, offline offers, and group calls remain excluded.

No automated tests were added. Manual validation uses the demo profile, native
compilation, and user-authorized live calls.

## Implementation references

The integration uses the pinned SDK's
[`Voip::call` / `Voip::accept`](https://github.com/oxidezap/whatsapp-rust/blob/b84f2af3d4029612a3c0cd45823bced1ee4614eb/src/client/voip.rs),
[`CallHandle`](https://github.com/oxidezap/whatsapp-rust/blob/b84f2af3d4029612a3c0cd45823bced1ee4614eb/src/voip/facade.rs),
and [audio/video endpoint contracts](https://github.com/oxidezap/whatsapp-rust/blob/b84f2af3d4029612a3c0cd45823bced1ee4614eb/wacore/src/voip_control/ports.rs).
The optional `voip` feature handles signaling, encryption, codecs for audio and
native relay transport. CPAL provides native PCM devices; Nokhwa captures the
camera; OpenH264 encodes/decodes video. Rune `Pixels` renders replaceable preview
frames without keeping an image cache entry for every frame.
