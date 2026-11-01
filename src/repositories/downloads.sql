-- Reservations precede filesystem effects; inode identity precedes publishing a filename.
-- No source paths, Telegram identifiers, display metadata or binary payloads are stored here.
CREATE TABLE download_records (
    download_id TEXT PRIMARY KEY CHECK(length(download_id) = 64),
    root_device TEXT NOT NULL CHECK(length(root_device) = 16),
    root_inode TEXT NOT NULL CHECK(length(root_inode) = 16),
    file_device TEXT CHECK(length(file_device) = 16),
    file_inode TEXT CHECK(length(file_inode) = 16),
    state TEXT NOT NULL CHECK(state IN ('reserved', 'writing', 'complete', 'retiring')),
    reserved_bytes INTEGER NOT NULL CHECK(reserved_bytes BETWEEN 1 AND 20000000),
    disk_bytes INTEGER NOT NULL CHECK(disk_bytes BETWEEN 1 AND 1073741824),
    actual_bytes INTEGER CHECK(actual_bytes BETWEEN 0 AND reserved_bytes),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    expires_at INTEGER NOT NULL CHECK(expires_at >= created_at),
    CHECK((state = 'reserved' AND file_device IS NULL AND file_inode IS NULL AND actual_bytes IS NULL)
       OR (state = 'writing' AND file_device IS NOT NULL AND file_inode IS NOT NULL AND actual_bytes IS NULL)
       OR (state = 'complete' AND file_device IS NOT NULL AND file_inode IS NOT NULL AND actual_bytes IS NOT NULL)
       OR (state = 'retiring' AND (file_device IS NULL) = (file_inode IS NULL)))
) STRICT;
CREATE INDEX download_expiry ON download_records(state, expires_at, download_id);
CREATE TRIGGER download_record_bound BEFORE INSERT ON download_records
    WHEN (SELECT count(*) FROM download_records) >= 1024
    BEGIN SELECT RAISE(ABORT, 'download capacity'); END;
