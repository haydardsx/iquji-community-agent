use crate::{AgentError, Attachment};
use iquji_agent_wire::{MAX_IMAGE_BYTES, validate_attachments};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub(crate) struct ImageBytes {
    pub(crate) descriptor: Attachment,
    pub(crate) bytes: Vec<u8>,
}
impl ImageBytes {
    pub(crate) fn media_type(&self) -> &str {
        let Attachment::Image { media_type, .. } = &self.descriptor;
        media_type
    }
}

pub(crate) fn load(
    attachments: &[Attachment],
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<Vec<ImageBytes>, AgentError> {
    validate_attachments(attachments)?;
    attachments
        .iter()
        .map(|image| {
            Ok(ImageBytes {
                descriptor: image.clone(),
                bytes: read(image, cancelled, deadline)?,
            })
        })
        .collect()
}

fn read(
    image: &Attachment,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<Vec<u8>, AgentError> {
    check(cancelled, deadline)?;
    let Attachment::Image {
        path,
        media_type,
        size,
        sha256,
    } = image;
    let path = Path::new(path);
    let before = std::fs::symlink_metadata(path).map_err(|_| AgentError::AttachmentUnavailable)?;
    if !valid_file(path, &before, *size) {
        return Err(AgentError::AttachmentChanged);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options
        .open(path)
        .map_err(|_| AgentError::AttachmentUnavailable)?;
    let opened = file
        .metadata()
        .map_err(|_| AgentError::AttachmentUnavailable)?;
    if !valid_file(path, &opened, *size) || !same_file(&before, &opened) {
        return Err(AgentError::AttachmentChanged);
    }
    let mut bytes = Vec::with_capacity(*size as usize);
    let mut buffer = [0u8; 8192];
    loop {
        check(cancelled, deadline)?;
        let maximum = buffer
            .len()
            .min((MAX_IMAGE_BYTES + 1).saturating_sub(bytes.len() as u64) as usize);
        if maximum == 0 {
            return Err(AgentError::AttachmentChanged);
        }
        let count = file
            .read(&mut buffer[..maximum])
            .map_err(|_| AgentError::AttachmentUnavailable)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let after = file
        .metadata()
        .map_err(|_| AgentError::AttachmentUnavailable)?;
    let current = std::fs::symlink_metadata(path).map_err(|_| AgentError::AttachmentUnavailable)?;
    if !valid_file(path, &after, *size)
        || !valid_file(path, &current, *size)
        || !same_file(&opened, &after)
        || !same_file(&opened, &current)
        || bytes.len() as u64 != *size
        || media_type_of(&bytes) != Some(media_type.as_str())
        || format!("{:x}", Sha256::digest(&bytes)) != *sha256
    {
        return Err(AgentError::AttachmentChanged);
    }
    check(cancelled, deadline)?;
    Ok(bytes)
}

fn media_type_of(bytes: &[u8]) -> Option<&'static str> {
    // 有界识别格式签名，不解压像素，也不宣称特定模型一定能解码此输入。
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else {
        None
    }
}
#[cfg(unix)]
fn valid_file(path: &Path, metadata: &std::fs::Metadata, size: u64) -> bool {
    use std::os::unix::fs::MetadataExt;
    let owner = unsafe { libc::geteuid() };
    metadata.is_file()
        && metadata.len() == size
        && metadata.uid() == owner
        && metadata.mode() & 0o7777 == 0o400
        && std::fs::canonicalize(path)
            .ok()
            .is_some_and(|real| real.as_os_str() == path.as_os_str())
        && path
            .parent()
            .and_then(|p| std::fs::symlink_metadata(p).ok())
            .is_some_and(|p| p.is_dir() && p.uid() == owner && p.mode() & 0o7777 == 0o700)
}
#[cfg(not(unix))]
fn valid_file(_: &Path, _: &std::fs::Metadata, _: u64) -> bool {
    false
}
#[cfg(unix)]
fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}
#[cfg(not(unix))]
fn same_file(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    false
}

pub(crate) fn check(cancelled: &AtomicBool, deadline: Instant) -> Result<(), AgentError> {
    if cancelled.load(Ordering::SeqCst) {
        Err(AgentError::TurnCancelled)
    } else if Instant::now() >= deadline {
        Err(AgentError::TurnTimedOut)
    } else {
        Ok(())
    }
}

pub(crate) struct StagedImages {
    _directory: crate::private_directory::PrivateDirectory,
    images: Vec<Attachment>,
}
impl StagedImages {
    pub(crate) fn new(
        images: &[ImageBytes],
        cancelled: &AtomicBool,
        deadline: Instant,
    ) -> Result<Self, AgentError> {
        let directory = crate::private_directory::PrivateDirectory::new()?;
        let mut staged = Self {
            _directory: directory,
            images: Vec::new(),
        };
        for (index, image) in images.iter().enumerate() {
            check(cancelled, deadline)?;
            let Attachment::Image {
                media_type,
                size,
                sha256,
                ..
            } = &image.descriptor;
            let extension = if media_type == "image/png" {
                "png"
            } else {
                "jpg"
            };
            let path = staged
                ._directory
                .path()
                .join(format!("{index}.{extension}"));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options
                .open(&path)
                .map_err(|_| AgentError::AttachmentUnavailable)?;
            file.write_all(&image.bytes)
                .map_err(|_| AgentError::AttachmentUnavailable)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o400))
                    .map_err(|_| AgentError::AttachmentUnavailable)?;
            }
            let descriptor = Attachment::Image {
                path: path.to_str().ok_or(AgentError::AttachmentInvalid)?.into(),
                media_type: media_type.clone(),
                size: *size,
                sha256: sha256.clone(),
            };
            read(&descriptor, cancelled, deadline)?;
            staged.images.push(descriptor);
        }
        Ok(staged)
    }
    pub(crate) fn paths(&self) -> Vec<&str> {
        self.images
            .iter()
            .map(|a| {
                let Attachment::Image { path, .. } = a;
                path.as_str()
            })
            .collect()
    }
    pub(crate) fn verify(
        &self,
        cancelled: &AtomicBool,
        deadline: Instant,
    ) -> Result<(), AgentError> {
        for image in &self.images {
            read(image, cancelled, deadline)?;
        }
        Ok(())
    }
}
