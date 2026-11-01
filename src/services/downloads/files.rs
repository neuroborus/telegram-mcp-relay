//! Pinned private roots and inode-checked publication/retirement. No untrusted filenames.

use crate::{
    error::AppError,
    repositories::{DownloadRecord, FileIdentity},
};
use rustix::{
    fs::{
        AtFlags, CWD, FallocateFlags, FlockOperation, Mode, OFlags, RenameFlags, ResolveFlags,
        fallocate, fchmod, flock, fstatvfs, linkat, open, openat2, renameat_with, unlinkat,
    },
    io::Errno,
    process::geteuid,
};
use std::{
    fs::{File, Metadata},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Component, Path, PathBuf},
};

pub(super) struct Root {
    pub path: PathBuf,
    directory: File,
    pub identity: FileIdentity,
}

fn unavailable() -> AppError {
    AppError::DownloadStorageUnavailable
}

pub(super) fn identity(metadata: &Metadata) -> FileIdentity {
    FileIdentity {
        device: format!("{:016x}", metadata.dev()),
        inode: format!("{:016x}", metadata.ino()),
    }
}

fn pin(path: &Path) -> Result<File, AppError> {
    if !path.is_absolute()
        || path.parent().is_none()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(unavailable());
    }
    let filesystem = open(
        "/",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| unavailable())?;
    let fd = openat2(
        filesystem,
        path.strip_prefix("/").map_err(|_| unavailable())?,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )
    .map_err(|_| unavailable())?;
    let file = File::from(fd);
    let metadata = file.metadata().map_err(|_| unavailable())?;
    if metadata.uid() != geteuid().as_raw() || metadata.mode() & 0o7777 != 0o700 {
        return Err(unavailable());
    }
    Ok(file)
}

impl Root {
    pub fn new(path: &Path) -> Result<Self, AppError> {
        let directory = pin(path)?;
        flock(&directory, FlockOperation::NonBlockingLockExclusive).map_err(|_| unavailable())?;
        let identity = identity(&directory.metadata().map_err(|_| unavailable())?);
        Ok(Self {
            path: path.to_owned(),
            directory,
            identity,
        })
    }

    pub fn current(&self) -> Result<(), AppError> {
        if identity(&pin(&self.path)?.metadata().map_err(|_| unavailable())?) != self.identity {
            return Err(unavailable());
        }
        Ok(())
    }

    pub fn available(&self) -> Result<(u64, u64, u64), AppError> {
        let stat = fstatvfs(&self.directory).map_err(|_| unavailable())?;
        Ok((
            stat.f_bavail.saturating_mul(stat.f_frsize),
            stat.f_frsize,
            stat.f_favail,
        ))
    }

    pub fn temporary(&self) -> Result<File, AppError> {
        self.current()?;
        // An unnamed inode disappears automatically on a crash before ownership is durable.
        let file = openat2(
            &self.directory,
            ".",
            OFlags::TMPFILE | OFlags::RDWR | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        )
        .map(File::from)
        .map_err(|_| unavailable())?;
        // Restore only this newly created unnamed inode's requested bits if umask removed
        // them. Never repair a preexisting file or configured directory's permissions.
        fchmod(&file, Mode::RUSR | Mode::WUSR).map_err(|_| unavailable())?;
        Ok(file)
    }

    pub fn allocate(file: &File, bytes: u64) -> Result<(), AppError> {
        // Keep the reserved EOF until completion; truncation then releases unused extents.
        fallocate(file, FallocateFlags::empty(), 0, bytes).map_err(|error| {
            if error == Errno::NOSPC || error == Errno::DQUOT {
                AppError::DownloadCapacity
            } else {
                unavailable()
            }
        })
    }

    pub fn publish(&self, record: &DownloadRecord, file: &File) -> Result<(), AppError> {
        self.current()?;
        // This kernel-owned descriptor link avoids privileged AT_EMPTY_PATH. No caller path
        // or source symlink is followed, and linkat never replaces an existing destination.
        let descriptor = format!("/proc/self/fd/{}", file.as_raw_fd());
        linkat(
            CWD,
            descriptor,
            &self.directory,
            filename(&record.id)?,
            AtFlags::SYMLINK_FOLLOW,
        )
        .map_err(|_| unavailable())?;
        self.verify(
            &filename(&record.id)?,
            &identity(&file.metadata().map_err(|_| unavailable())?),
        )?;
        self.directory.sync_all().map_err(|_| unavailable())
    }

    fn opened(&self, name: &str) -> Result<Option<File>, AppError> {
        match openat2(
            &self.directory,
            name,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
        ) {
            Ok(fd) => Ok(Some(File::from(fd))),
            Err(Errno::NOENT) => Ok(None),
            Err(_) => Err(unavailable()),
        }
    }

    pub fn verify(&self, name: &str, expected: &FileIdentity) -> Result<File, AppError> {
        let file = self.opened(name)?.ok_or_else(unavailable)?;
        let metadata = file.metadata().map_err(|_| unavailable())?;
        if !metadata.is_file()
            || metadata.mode() & 0o7777 != 0o600
            || metadata.uid() != geteuid().as_raw()
            || metadata.nlink() != 1
            || identity(&metadata) != *expected
        {
            return Err(unavailable());
        }
        Ok(file)
    }

    pub fn remove(&self, record: &DownloadRecord) -> Result<(), AppError> {
        let name = filename(&record.id)?;
        let retiring = format!(".retiring-{}", record.id);
        if self.opened(&retiring)?.is_none() {
            if self.opened(&name)?.is_none() {
                return self.directory.sync_all().map_err(|_| unavailable());
            }
            let expected = record.file.as_ref().ok_or_else(unavailable)?;
            self.verify(&name, expected)?;
            renameat_with(
                &self.directory,
                &name,
                &self.directory,
                &retiring,
                RenameFlags::NOREPLACE,
            )
            .map_err(|_| unavailable())?;
            if self.verify(&retiring, expected).is_err() {
                // A concurrent replacement is preserved, never overwritten or deleted.
                let _ = renameat_with(
                    &self.directory,
                    &retiring,
                    &self.directory,
                    &name,
                    RenameFlags::NOREPLACE,
                );
                return Err(unavailable());
            }
        }
        self.verify(&retiring, record.file.as_ref().ok_or_else(unavailable)?)?;
        unlinkat(&self.directory, retiring, AtFlags::empty()).map_err(|_| unavailable())?;
        self.directory.sync_all().map_err(|_| unavailable())
    }
}

pub(super) fn filename(id: &str) -> Result<String, AppError> {
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(unavailable());
    }
    Ok(format!("relay-download-{id}.bin"))
}
