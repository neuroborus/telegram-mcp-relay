//! Private bounded file ownership and reservations, independent of history opt-in.

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::{Database, registry::random_key};
use crate::{config::Downloads, error::AppError};

// Matches the disk_bytes bound in download_sizes.sql, including allocation metadata.
const MAX_STORED_DISK_BYTES: u64 = 64 * (1 << 30);

/// Return block-rounded data bytes and the disk bytes reserved for data and metadata.
/// At 4 KiB, an ext4 leaf holds 340 extents. One metadata block per 256 data
/// blocks plus one extra leaves room for extent-tree indexes as well as leaves.
/// Other filesystems still have to fit the reservation when allocation is measured.
fn reservation_sizes(bytes: u64, allocation_unit: u64) -> Result<(u64, u64), AppError> {
    let unit = allocation_unit.max(512);
    let data_units = bytes.div_ceil(unit);
    let data_bytes = data_units
        .checked_mul(unit)
        .ok_or(AppError::DownloadCapacity)?;
    let metadata_bytes = data_units
        .div_ceil(256)
        .checked_add(1)
        .and_then(|blocks| blocks.checked_mul(unit))
        .ok_or(AppError::DownloadCapacity)?;
    let disk_bytes = data_bytes
        .checked_add(metadata_bytes)
        .ok_or(AppError::DownloadCapacity)?;
    Ok((data_bytes, disk_bytes))
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    pub device: String,
    pub inode: String,
}

#[derive(Clone)]
pub(crate) struct DownloadRecord {
    pub id: String,
    pub root: FileIdentity,
    pub file: Option<FileIdentity>,
    pub complete: bool,
    pub retiring: bool,
    pub reserved_bytes: u64,
    pub disk_bytes: u64,
    pub actual_bytes: Option<u64>,
    pub expires_at: i64,
}

impl std::fmt::Debug for FileIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileIdentity([REDACTED])")
    }
}
impl std::fmt::Debug for DownloadRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DownloadRecord([REDACTED])")
    }
}

impl Database {
    pub(crate) fn cap_download_expiry(
        &mut self,
        id: &str,
        expiry: i64,
        now: i64,
    ) -> Result<i64, AppError> {
        self.connection.query_row(
            "UPDATE download_records SET expires_at = min(expires_at, ?2) WHERE download_id = ?1
             AND state = 'complete' AND expires_at > ?3 AND ?2 > ?3 RETURNING expires_at",
            params![id, expiry, now], |row| row.get(0)).optional()?.ok_or(AppError::AttachmentUnavailable)
    }

    pub(crate) fn reserve_download(
        &mut self,
        root: &FileIdentity,
        policy: &Downloads,
        retained_size: Option<u64>,
        now: i64,
        (available_bytes, allocation_unit, available_files): (u64, u64, u64),
    ) -> Result<DownloadRecord, AppError> {
        let expires_at = now
            .checked_add((policy.timeout_seconds * 1_000) as i64)
            .ok_or(AppError::InvalidRequest("download timestamp"))?;
        if now < 0 {
            return Err(AppError::InvalidRequest("download timestamp"));
        }
        let reserved_bytes = retained_size
            .unwrap_or(policy.max_file_bytes)
            .min(policy.max_file_bytes);
        // Even an empty file needs an owned inode and a bounded temporary allocation.
        let (_, disk_bytes) = reservation_sizes(reserved_bytes.max(1), allocation_unit)?;
        if disk_bytes > MAX_STORED_DISK_BYTES {
            return Err(AppError::DownloadCapacity);
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (count, reserved, active): (u64, u64, u64) = transaction.query_row(
            "SELECT count(*), coalesce(sum(coalesce(actual_bytes, reserved_bytes)), 0),
                    coalesce(sum(state != 'complete'), 0) FROM download_records",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let (pending, pending_files): (u64, u64) = transaction.query_row("SELECT coalesce(sum(disk_bytes), 0), count(*) FROM download_records WHERE state != 'complete' AND root_device = ?1", [&root.device], |row| Ok((row.get(0)?, row.get(1)?)))?;
        if count >= u64::from(policy.max_files)
            || active >= u64::from(policy.concurrent)
            || reserved.saturating_add(reserved_bytes) > policy.max_total_bytes
            || pending_files.saturating_add(1) > available_files
            || pending
                .saturating_add(disk_bytes)
                .saturating_add(policy.min_free_bytes)
                > available_bytes
        {
            return Err(AppError::DownloadCapacity);
        }
        let record = DownloadRecord {
            id: random_key()?,
            root: root.clone(),
            file: None,
            complete: false,
            retiring: false,
            reserved_bytes,
            disk_bytes,
            actual_bytes: None,
            expires_at,
        };
        transaction.execute("INSERT INTO download_records(download_id, root_device, root_inode, state, reserved_bytes, disk_bytes, created_at, expires_at) VALUES(?1, ?2, ?3, 'reserved', ?4, ?5, ?6, ?7)",
            params![record.id, root.device, root.inode, reserved_bytes, disk_bytes, now, expires_at])?;
        transaction.commit()?;
        Ok(record)
    }

    pub(crate) fn own_download_inode(
        &mut self,
        id: &str,
        file: &FileIdentity,
    ) -> Result<(), AppError> {
        let changed = self.connection.execute("UPDATE download_records SET file_device = ?2, file_inode = ?3, state = 'writing' WHERE download_id = ?1 AND state = 'reserved'", params![id, file.device, file.inode])?;
        if changed != 1 {
            return Err(AppError::StorageFailure);
        }
        Ok(())
    }

    pub(crate) fn complete_download(
        &mut self,
        id: &str,
        bytes: u64,
        disk_bytes: u64,
        now: i64,
        retention_seconds: u64,
    ) -> Result<DownloadRecord, AppError> {
        let expires = now
            .checked_add(
                i64::try_from(
                    retention_seconds
                        .checked_mul(1_000)
                        .ok_or(AppError::StorageFailure)?,
                )
                .map_err(|_| AppError::StorageFailure)?,
            )
            .ok_or(AppError::StorageFailure)?;
        let changed = self.connection.execute("UPDATE download_records SET state = 'complete', actual_bytes = ?2, reserved_bytes = max(?2, 1), disk_bytes = max(?4, 1), expires_at = ?3 WHERE download_id = ?1 AND state = 'writing' AND ?2 <= reserved_bytes AND ?4 <= disk_bytes", params![id, bytes, expires, disk_bytes])?;
        if changed != 1 {
            return Err(AppError::StorageFailure);
        }
        self.download_record(id)?.ok_or(AppError::StorageFailure)
    }

    pub(crate) fn download_records(&self, after: &str) -> Result<Vec<DownloadRecord>, AppError> {
        let mut statement = self.connection.prepare_cached("SELECT download_id, root_device, root_inode, file_device, file_inode, state, reserved_bytes, actual_bytes, expires_at, disk_bytes FROM download_records WHERE download_id > ?1 ORDER BY download_id LIMIT 100")?;
        let rows = statement.query_map([after], record)?;
        rows.collect::<Result<_, _>>().map_err(Into::into)
    }

    pub(crate) fn download_record(&self, id: &str) -> Result<Option<DownloadRecord>, AppError> {
        self.connection.query_row("SELECT download_id, root_device, root_inode, file_device, file_inode, state, reserved_bytes, actual_bytes, expires_at, disk_bytes FROM download_records WHERE download_id = ?1", [id], record).optional().map_err(Into::into)
    }

    pub(crate) fn retire_download(&mut self, id: &str) -> Result<(), AppError> {
        self.connection.execute(
            "UPDATE download_records SET state = 'retiring' WHERE download_id = ?1",
            [id],
        )?;
        Ok(())
    }

    pub(crate) fn forget_download(&mut self, id: &str) -> Result<(), AppError> {
        self.connection
            .execute("DELETE FROM download_records WHERE download_id = ?1", [id])?;
        Ok(())
    }
}

fn record(row: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadRecord> {
    let device: Option<String> = row.get(3)?;
    let inode: Option<String> = row.get(4)?;
    let state: String = row.get(5)?;
    Ok(DownloadRecord {
        id: row.get(0)?,
        root: FileIdentity {
            device: row.get(1)?,
            inode: row.get(2)?,
        },
        file: device
            .zip(inode)
            .map(|(device, inode)| FileIdentity { device, inode }),
        complete: state == "complete",
        retiring: state == "retiring",
        reserved_bytes: row.get(6)?,
        actual_bytes: row.get(7)?,
        expires_at: row.get(8)?,
        disk_bytes: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservation_sizes_round_data_and_allow_for_extent_metadata() {
        assert_eq!(reservation_sizes(1, 0), Ok((512, 1_536)));
        assert_eq!(
            reservation_sizes(20_000_000, 4_096),
            Ok((20_000_768, 20_086_784))
        );
        assert_eq!(
            reservation_sizes(4 * 1024_u64.pow(3), 4_096),
            Ok((4_294_967_296, 4_311_748_608))
        );
    }

    #[test]
    fn reservation_sizes_reject_overflow_in_data_metadata_and_total() {
        for (bytes, unit) in [(u64::MAX, 4_096), (1, u64::MAX), (u64::MAX - 511, 512)] {
            assert_eq!(
                reservation_sizes(bytes, unit),
                Err(AppError::DownloadCapacity)
            );
        }
    }
}
