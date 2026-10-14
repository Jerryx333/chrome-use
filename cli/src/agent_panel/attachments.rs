//! Immutable, bounded image files belong to a conversation, never to a page.
use super::config_transaction::{atomic_write, private_open, private_tree};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub(super) const MAX_IMAGES: usize = 5;
const MAX_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Attachment {
    pub id: String,
    pub media_type: String,
    pub bytes: usize,
}
impl Attachment {
    pub fn path(&self, workspace: &Path) -> Result<PathBuf, String> {
        if self.id.len() != 64 || !self.id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid attachment identity".into());
        }
        let extension = match self.media_type.as_str() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            _ => return Err("Unsupported attachment image type".into()),
        };
        Ok(workspace
            .join("attachments")
            .join(format!("{}.{}", self.id, extension)))
    }
    pub fn read(&self, workspace: &Path) -> Result<Vec<u8>, String> {
        let path = self.path(workspace)?;
        let mut file = private_open(&path, false)?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "Cannot read attachment")?;
        if bytes.len() > MAX_BYTES
            || bytes.len() != self.bytes
            || hex::encode(Sha256::digest(&bytes)) != self.id
        {
            return Err("The saved attachment changed; it was not sent".into());
        }
        Ok(bytes)
    }
    pub fn base64(&self, workspace: &Path) -> Result<String, String> {
        Ok(STANDARD.encode(self.read(workspace)?))
    }
}

/// Decode and verify image dimensions before any model receives the file.
pub(super) fn save(workspace: &Path, data: &str) -> Result<Attachment, String> {
    if data.len() > MAX_BYTES.div_ceil(3) * 4 {
        return Err("Image exceeds 512 KiB; resize it before attaching".into());
    }
    let bytes = STANDARD
        .decode(data)
        .map_err(|_| "Invalid attachment encoding")?;
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err("Image exceeds the attachment limit".into());
    }
    let format = image::guess_format(&bytes).map_err(|_| "Attachment is not a supported image")?;
    let media_type = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::WebP => "image/webp",
        _ => return Err("Use PNG, JPEG or WebP images".into()),
    };
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .map_err(|_| "Image is invalid or exceeds 4096 pixels per side")?;
    let attachment = Attachment {
        id: hex::encode(Sha256::digest(&bytes)),
        media_type: media_type.into(),
        bytes: bytes.len(),
    };
    private_tree(&workspace.join("attachments"))?;
    let path = attachment.path(workspace)?;
    if path.exists() {
        attachment.read(workspace)?;
    } else {
        atomic_write(&path, &bytes, None)?;
    }
    Ok(attachment)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png() -> Vec<u8> {
        let image = image::DynamicImage::new_rgb8(2, 2);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }
    #[test]
    fn images_are_content_addressed_private_and_change_detected() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("workspace");
        let encoded = STANDARD.encode(png());
        let a = save(&dir, &encoded).unwrap();
        assert_eq!(a.id, save(&dir, &encoded).unwrap().id);
        assert_eq!(a.read(&dir).unwrap(), png());
        std::fs::write(a.path(&dir).unwrap(), b"changed").unwrap();
        assert!(a.read(&dir).is_err());
        assert!(save(&dir, &STANDARD.encode(b"not an image")).is_err());
        let mut forged = a;
        forged.id = "../other".into();
        assert!(forged.path(&dir).is_err());
    }
}
