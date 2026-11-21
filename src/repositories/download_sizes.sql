-- Widen the byte bounds while preserving ownership and recovery for every existing row.
-- The v10 trigger limits the copy to 1024 rows, and startup sets temp_store=MEMORY.
-- Release the old table's pages before rebuilding under the main database page cap.
CREATE TEMP TABLE saved_download_records AS
    SELECT rowid AS record_rowid, * FROM main.download_records;
DROP TABLE main.download_records;
CREATE TABLE download_records (
    download_id TEXT NOT NULL CHECK(length(download_id) = 64),
    root_device TEXT NOT NULL CHECK(length(root_device) = 16),
    root_inode TEXT NOT NULL CHECK(length(root_inode) = 16),
    file_device TEXT CHECK(length(file_device) = 16),
    file_inode TEXT CHECK(length(file_inode) = 16),
    state TEXT NOT NULL CHECK(state IN ('reserved', 'writing', 'complete', 'retiring')),
    reserved_bytes INTEGER NOT NULL CHECK(reserved_bytes BETWEEN 0 AND 4294967296),
    disk_bytes INTEGER NOT NULL CHECK(disk_bytes BETWEEN 1 AND 68719476736),
    actual_bytes INTEGER CHECK(actual_bytes BETWEEN 0 AND reserved_bytes),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    expires_at INTEGER NOT NULL CHECK(expires_at >= created_at),
    CHECK((state = 'reserved' AND file_device IS NULL AND file_inode IS NULL AND actual_bytes IS NULL)
       OR (state = 'writing' AND file_device IS NOT NULL AND file_inode IS NOT NULL AND actual_bytes IS NULL)
       OR (state = 'complete' AND file_device IS NOT NULL AND file_inode IS NOT NULL AND actual_bytes IS NOT NULL)
       OR (state = 'retiring' AND (file_device IS NULL) = (file_inode IS NULL)))
) STRICT;
-- Preserve the row order and build packed indexes after copying the data.
INSERT INTO download_records (rowid, download_id, root_device, root_inode, file_device,
                              file_inode, state, reserved_bytes, disk_bytes, actual_bytes,
                              created_at, expires_at)
    SELECT record_rowid, download_id, root_device, root_inode, file_device, file_inode,
           state, reserved_bytes, disk_bytes, actual_bytes, created_at, expires_at
    FROM temp.saved_download_records ORDER BY record_rowid;
DROP TABLE temp.saved_download_records;
CREATE UNIQUE INDEX download_identity ON download_records(download_id);
CREATE INDEX download_expiry ON download_records(state, expires_at, download_id);
CREATE TRIGGER download_record_bound BEFORE INSERT ON download_records
    WHEN (SELECT count(*) FROM download_records) >= 1024
    BEGIN SELECT RAISE(ABORT, 'download capacity'); END;
