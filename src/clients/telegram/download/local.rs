//! Read-only access to files materialized by a local Bot API.

use std::{
    fs::File,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use rustix::fs::{Mode, OFlags, ResolveFlags, open, openat2};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio_util::task::TaskTracker;

use super::{WRITE_CHUNK_BYTES, check_size};
use crate::{
    config::LOCAL_BOT_API_FILES_ROOT,
    error::{AppError, TelegramError},
};

const CONFINED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS);

pub(in crate::clients::telegram) struct LocalFiles {
    path: PathBuf,
    directory: File,
    identity: (u64, u64),
}

fn directory(path: &Path) -> Option<File> {
    let relative = path.strip_prefix("/").ok()?;
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return None;
    }
    let filesystem = open(
        "/",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    openat2(
        filesystem,
        relative,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        CONFINED,
    )
    .ok()
    .map(File::from)
}

fn identity(directory: &File) -> Option<(u64, u64)> {
    let stat = directory.metadata().ok()?;
    Some((stat.dev(), stat.ino()))
}

impl LocalFiles {
    pub(in crate::clients::telegram) fn pin(path: &Path) -> Result<Self, AppError> {
        let invalid = || AppError::InvalidConfig(LOCAL_BOT_API_FILES_ROOT);
        let directory = directory(path).ok_or_else(invalid)?;
        let identity = identity(&directory).ok_or_else(invalid)?;
        Ok(Self {
            path: path.to_owned(),
            directory,
            identity,
        })
    }

    fn current(&self) -> Result<(), TelegramError> {
        if directory(&self.path).as_ref().and_then(identity) != Some(self.identity) {
            return Err(TelegramError::InvalidResponse);
        }
        Ok(())
    }

    fn open_file(
        &self,
        path: &str,
        reported_size: Option<u64>,
        limit: u64,
    ) -> Result<(File, u64), AppError> {
        let invalid = || TelegramError::InvalidResponse;
        let components = path.strip_prefix('/').ok_or_else(invalid)?;
        if path.len() > 4_096
            || path.as_bytes().contains(&0)
            || components
                .split('/')
                .any(|part| matches!(part, "" | "." | ".."))
        {
            return Err(invalid().into());
        }
        let relative = Path::new(path)
            .strip_prefix(&self.path)
            .ok()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or_else(invalid)?;
        if let Some(size) = reported_size {
            check_size(size, limit)?;
        }
        self.current()?;
        let resolve = CONFINED | ResolveFlags::NO_XDEV;
        // Reject special files before opening for data access. Keep the inspected
        // inode pinned so a replacement cannot reuse its identity before the check.
        let entry = openat2(
            &self.directory,
            relative,
            OFlags::PATH | OFlags::CLOEXEC,
            Mode::empty(),
            resolve,
        )
        .map(File::from)
        .map_err(|_| invalid())?;
        let entry_stat = entry.metadata().map_err(|_| invalid())?;
        if !entry_stat.is_file() {
            return Err(invalid().into());
        }

        // NONBLOCK avoids waiting on a substituted FIFO. NOCTTY prevents tty acquisition.
        let file = openat2(
            &self.directory,
            relative,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK | OFlags::NOCTTY,
            Mode::empty(),
            resolve,
        )
        .map(File::from)
        .map_err(|_| invalid())?;
        let stat = file.metadata().map_err(|_| invalid())?;
        if !stat.is_file() || (stat.dev(), stat.ino()) != (entry_stat.dev(), entry_stat.ino()) {
            return Err(invalid().into());
        }
        let size = stat.len();
        check_size(size, limit)?;
        if reported_size.is_some_and(|reported| reported != size) {
            return Err(invalid().into());
        }
        Ok((file, size))
    }

    pub(super) async fn copy<W: AsyncWrite + Unpin>(
        self: &Arc<Self>,
        path: String,
        reported_size: Option<u64>,
        limit: u64,
        output: &mut W,
        source_io: &TaskTracker,
    ) -> Result<u64, AppError> {
        let root = self.clone();
        let (mut file, size) = source_io
            .spawn_blocking(move || root.open_file(&path, reported_size, limit))
            .await
            .map_err(|_| AppError::DownloadUnavailable)??;
        let mut chunk = vec![0; WRITE_CHUNK_BYTES];
        let mut total = 0;
        loop {
            // Keep the descriptor and buffer in the tracked operation. A canceled
            // transfer cannot interrupt a kernel read or leave it outside admission.
            let (reader, buffer, count) = source_io
                .spawn_blocking(move || {
                    let result = file.read(&mut chunk);
                    result.map(|count| (file, chunk, count))
                })
                .await
                .map_err(|_| AppError::DownloadUnavailable)?
                .map_err(|_| AppError::DownloadUnavailable)?;
            file = reader;
            chunk = buffer;
            if count == 0 {
                break;
            }
            total += count as u64;
            check_size(total, limit)?;
            if total > size {
                return Err(TelegramError::InvalidResponse.into());
            }
            output
                .write_all(&chunk[..count])
                .await
                .map_err(|_| AppError::DownloadWriteFailed)?;
        }
        if total != size {
            return Err(TelegramError::InvalidResponse.into());
        }
        output
            .flush()
            .await
            .map_err(|_| AppError::DownloadWriteFailed)?;
        let root = self.clone();
        source_io
            .spawn_blocking(move || root.current())
            .await
            .map_err(|_| AppError::DownloadUnavailable)??;
        Ok(total)
    }
}
