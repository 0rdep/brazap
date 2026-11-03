use crate::model::Message;
use anyhow::{Context, Result, bail};
use whatsapp_rust::download::{ClientDownloadError, DownloadLimits, Downloadable, MediaType};
use whatsapp_rust::{Client, MediaRetryResult, MediaReuploadRequest, MessageRef};

// Preserve the original encryption key and integrity hashes. Only the expired
// CDN reference changes; retained message protobufs remain untouched.
struct Refreshed<'a> {
    original: &'a dyn Downloadable,
    path: String,
}
impl Downloadable for Refreshed<'_> {
    fn direct_path(&self) -> Option<&str> {
        Some(&self.path)
    }
    fn media_key(&self) -> Option<&[u8]> {
        self.original.media_key()
    }
    fn file_enc_sha256(&self) -> Option<&[u8]> {
        self.original.file_enc_sha256()
    }
    fn file_sha256(&self) -> Option<&[u8]> {
        self.original.file_sha256()
    }
    fn file_length(&self) -> Option<u64> {
        self.original.file_length()
    }
    fn app_info(&self) -> MediaType {
        self.original.app_info()
    }
}

pub(crate) async fn download(
    client: &Client,
    message: &Message,
    media: &dyn Downloadable,
) -> Result<Vec<u8>> {
    let limits = DownloadLimits::new(128 * 1024 * 1024, 64 * 1024 * 1024);
    match client.download(media, limits).await {
        Ok(bytes) => return Ok(bytes),
        Err(ClientDownloadError::ReferenceRejected(_)) => {}
        Err(error) => return Err(error.into()),
    }
    let key: &[u8; 32] = media
        .media_key()
        .context("This attachment link expired and cannot be renewed on this device.")?
        .try_into()
        .context("This attachment has an invalid encryption key.")?;
    let chat = message.chat.parse()?;
    let sender = if message.sender == "self" {
        None
    } else {
        Some(message.sender.parse::<whatsapp_rust::Jid>()?)
    };
    // Some media entry points project the saved payload without its from_me
    // flag. An own PN/LID still identifies our original upload accurately.
    let from_me = message.from_me
        || message.sender == "self"
        || sender.as_ref().is_some_and(|sender| {
            [client.pn(), client.lid()]
                .iter()
                .flatten()
                .any(|own| own.to_non_ad() == sender.to_non_ad())
        });
    let target = MessageRef::new(
        &chat,
        whatsapp_rust::MessageId::new(&message.id)?,
        sender.as_ref(),
        from_me,
    )?;
    let result = client
        .media_reupload()
        .request(&MediaReuploadRequest {
            target,
            media_key: key,
        })
        .await;
    let path = match result {
        Ok(MediaRetryResult::Success { direct_path }) => direct_path,
        Ok(MediaRetryResult::NotFound) => bail!(
            "WhatsApp could not restore this attachment. It may no longer be available on your phone."
        ),
        Ok(_) => {
            bail!("WhatsApp could not restore this attachment. Try again with your phone online.")
        }
        Err(error) => {
            log::debug!("Attachment renewal failed: {error:?}");
            bail!(
                "Could not renew this attachment's download link. Keep WhatsApp online on your phone and try again."
            );
        }
    };
    client
        .download(
            &Refreshed {
                original: media,
                path,
            },
            limits,
        )
        .await
        .context("Could not download the restored attachment")
}
