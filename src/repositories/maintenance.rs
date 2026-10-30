//! Bounded incremental reclamation without a second database copy or disk-backed temp work.

use std::{ffi::OsString, fs::File, os::unix::fs::MetadataExt};

use rusqlite::TransactionBehavior;
use rustix::fs::{Mode, OFlags, openat};

use super::{Database, check_file};
use crate::error::AppError;

/// Maximum free-list pages removed by one maintenance transaction. Pointer-map pages may
/// also disappear at a boundary; this is a work bound, not an exact file-length decrement.
pub(crate) const RECLAIM_PAGES: u32 = 32;

pub(super) struct DatabaseFiles {
    pub database: File,
    pub directory: File,
    pub journal_name: OsString,
}

/// A point-in-time snapshot, not a filesystem quota or a peak-transaction measurement.
/// Allocations use Unix 512-byte block counts; filesystem metadata/snapshots are excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskUsage {
    pub database_bytes: u64,
    pub database_allocated_bytes: u64,
    pub journal_bytes: u64,
    pub journal_allocated_bytes: u64,
    /// Zero: temp_store=MEMORY covers query work and statement subjournals; no full VACUUM,
    /// attached databases or outbound file copies are used. The rollback journal is separate.
    pub temporary_disk_bytes: u64,
    pub page_size: u32,
    pub free_pages: u32,
    /// Configured main-file cap rounded down to whole pages; excludes the rollback journal.
    pub main_file_limit_bytes: u64,
}

impl Database {
    /// Account only for the pinned main file and its protected, descriptor-relative journal.
    /// A missing DELETE journal counts as zero; inaccessible/unsafe sidecars are errors.
    /// Same-user replacement and external writers are outside the trusted single-owner model.
    pub fn disk_usage(&self) -> Result<DiskUsage, AppError> {
        let database = self
            .files
            .database
            .metadata()
            .map_err(|_| AppError::StorageUnavailable)?;
        let (journal_bytes, journal_allocated_bytes) = match openat(
            &self.files.directory,
            &self.files.journal_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => {
                let journal = File::from(fd);
                check_file(&journal)?;
                let metadata = journal
                    .metadata()
                    .map_err(|_| AppError::StorageUnavailable)?;
                (metadata.len(), metadata.blocks() * 512)
            }
            Err(rustix::io::Errno::NOENT) => (0, 0),
            Err(_) => return Err(AppError::StorageUnavailable),
        };
        Ok(DiskUsage {
            database_bytes: database.len(),
            database_allocated_bytes: database.blocks() * 512,
            journal_bytes,
            journal_allocated_bytes,
            temporary_disk_bytes: 0,
            page_size: self
                .connection
                .pragma_query_value(None, "page_size", |r| r.get(0))?,
            free_pages: self
                .connection
                .pragma_query_value(None, "freelist_count", |r| r.get(0))?,
            main_file_limit_bytes: self.main_file_limit_bytes,
        })
    }

    /// Commit at most 32 incremental-vacuum steps. This neither expires nor deletes records.
    /// Call between logical cleanup batches, never inside their transaction; failure cannot
    /// undo already committed deletions or their post-commit notifications/typing updates.
    pub fn reclaim_batch(&mut self) -> Result<DiskUsage, AppError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut statement =
                transaction.prepare(&format!("PRAGMA incremental_vacuum({RECLAIM_PAGES})"))?;
            let mut rows = statement.query([])?;
            // SQLite returns one row per step. execute_batch would perform only one step.
            while rows.next()?.is_some() {}
        }
        transaction.commit()?;
        self.disk_usage()
    }
}
