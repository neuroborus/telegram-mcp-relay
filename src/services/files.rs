//! Descriptor-confined outbound files; never reopen a validated pathname for upload.

use std::{
    fmt,
    fs::File,
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
};

use rustix::fs::{Mode, OFlags, ResolveFlags, open, openat2};

use crate::{config::Files, error::AppError};

// Telegram's cloud sendDocument limit is 50 MB. Use decimal MB conservatively.
const CLOUD_DOCUMENT_BYTES: u64 = 50_000_000;

/// Pins at most 64 configured roots. All path components must be free of symlinks.
/// Linux openat2 (5.6+) is required; unsupported or denied resolution fails closed.
pub struct OutboundFiles {
    roots: Vec<(PathBuf, File)>,
    limit_bytes: u64,
}

/// Owns the checked, readable descriptor and bounded upload metadata, without a source path.
/// The size is a snapshot, not a guarantee against later in-place writes or truncation.
pub struct ValidatedFile {
    file: File,
    filename: String,
    size_bytes: u64,
}

impl fmt::Debug for OutboundFiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OutboundFiles([REDACTED])")
    }
}

impl fmt::Debug for ValidatedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ValidatedFile([REDACTED])")
    }
}

impl OutboundFiles {
    /// Open existing roots without creating directories, copying data or changing permissions.
    pub fn new(config: &Files) -> Result<Self, AppError> {
        let limit_bytes = u64::from(config.max_outbound_mb) * 1_000_000;
        if limit_bytes == 0 || limit_bytes > CLOUD_DOCUMENT_BYTES {
            return Err(AppError::InvalidConfig("files.max_outbound_mb"));
        }
        if config.allowed_roots.is_empty() || config.allowed_roots.len() > 64 {
            return Err(AppError::InvalidConfig("files.allowed_roots"));
        }
        let filesystem_root = open(
            "/",
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| AppError::FileUnavailable)?;
        let mut roots = Vec::with_capacity(config.allowed_roots.len());
        for path in &config.allowed_roots {
            if !valid_path(path) || path.parent().is_none() {
                return Err(AppError::InvalidConfig("files.allowed_roots"));
            }
            let relative = path
                .strip_prefix("/")
                .map_err(|_| AppError::InvalidConfig("files.allowed_roots"))?;
            let directory =
                open_beneath(&filesystem_root, relative, OFlags::PATH | OFlags::DIRECTORY)?;
            roots.push((path.clone(), directory));
        }
        Ok(Self { roots, limit_bytes })
    }

    /// Resolve the absolute caller path in one kernel-confined lookup below a pinned root.
    /// Return the same descriptor that passed type, readability and size checks.
    pub fn open(&self, path: &Path) -> Result<ValidatedFile, AppError> {
        if !valid_path(path) {
            return Err(AppError::FileUnavailable);
        }
        // Prefix matching is component based: /exports-other does not match /exports.
        let (relative, root) = self
            .roots
            .iter()
            .find_map(|(prefix, root)| path.strip_prefix(prefix).ok().map(|p| (p, root)))
            .ok_or(AppError::FileUnavailable)?;
        if relative.as_os_str().is_empty() {
            return Err(AppError::FileUnavailable);
        }
        // NONBLOCK prevents a FIFO substitution from hanging before the type check.
        let file = open_beneath(root, relative, OFlags::RDONLY | OFlags::NONBLOCK)?;
        let metadata = file.metadata().map_err(|_| AppError::FileUnavailable)?;
        // Even an elevated daemon must not export a file with all read bits cleared.
        if !metadata.is_file() || metadata.permissions().mode() & 0o444 == 0 {
            return Err(AppError::FileUnavailable);
        }
        if metadata.len() > self.limit_bytes {
            return Err(AppError::FileTooLarge {
                limit_bytes: self.limit_bytes,
                actual_bytes: metadata.len(),
            });
        }
        Ok(ValidatedFile {
            file,
            filename: safe_filename(path),
            size_bytes: metadata.len(),
        })
    }
}

impl ValidatedFile {
    pub fn filename(&self) -> &str {
        &self.filename
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// Transfer the original descriptor, sanitized basename and opening size to the uploader.
    /// The uploader must bound reads to this size even if the source subsequently grows.
    pub fn into_parts(self) -> (File, String, u64) {
        (self.file, self.filename, self.size_bytes)
    }
}

fn open_beneath(
    root: impl std::os::fd::AsFd,
    relative: &Path,
    flags: OFlags,
) -> Result<File, AppError> {
    openat2(
        root,
        relative,
        flags | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )
    .map(File::from)
    .map_err(|_| AppError::FileUnavailable)
}

fn valid_path(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        && !path.as_os_str().as_encoded_bytes().contains(&0)
}

fn safe_filename(path: &Path) -> String {
    let name: String = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect();
    let name = name.trim_matches('.');
    if name.is_empty() {
        "document".to_owned()
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests;
