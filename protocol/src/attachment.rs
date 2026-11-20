use crate::AgentError;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MAX_IMAGES: usize = 3;
pub const MAX_IMAGE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Attachment {
    Image {
        path: String,
        media_type: String,
        size: u64,
        sha256: String,
    },
}

pub fn validate_attachments(attachments: &[Attachment]) -> Result<(), AgentError> {
    if attachments.len() > MAX_IMAGES {
        return Err(AgentError::AttachmentInvalid);
    }
    for Attachment::Image {
        path,
        media_type,
        size,
        sha256,
    } in attachments
    {
        if path.is_empty()
            || path.len() > 4096
            || path.chars().any(char::is_control)
            || !Path::new(path).is_absolute()
            || Path::new(path).components().any(|part| {
                matches!(
                    part,
                    std::path::Component::CurDir | std::path::Component::ParentDir
                )
            })
            || !matches!(media_type.as_str(), "image/png" | "image/jpeg")
            || !(1..=MAX_IMAGE_BYTES).contains(size)
            || sha256.len() != 64
            || !sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(AgentError::AttachmentInvalid);
        }
    }
    Ok(())
}
