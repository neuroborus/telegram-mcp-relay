-- Shared observations never contain credentials, claim tokens, paths or file bytes.
CREATE TABLE history_usage (
    singleton_id INTEGER PRIMARY KEY CHECK(singleton_id = 1),
    records INTEGER NOT NULL DEFAULT 0 CHECK(records >= 0),
    attachments INTEGER NOT NULL DEFAULT 0 CHECK(attachments >= 0),
    body_bytes INTEGER NOT NULL DEFAULT 0 CHECK(body_bytes >= 0),
    metadata_bytes INTEGER NOT NULL DEFAULT 0 CHECK(metadata_bytes >= 0)
) STRICT;
INSERT INTO history_usage(singleton_id) VALUES(1);
CREATE TABLE history_coverage (
    singleton_id INTEGER PRIMARY KEY CHECK(singleton_id = 1),
    observed_from INTEGER CHECK(observed_from >= 0),
    observed_through INTEGER CHECK(observed_through >= observed_from),
    has_gaps INTEGER NOT NULL DEFAULT 0 CHECK(has_gaps IN (0, 1))
) STRICT;
INSERT INTO history_coverage(singleton_id) VALUES(1);

-- Allocate lazily. Journal snapshots survive expiry of the private mapping/session.
CREATE TABLE session_displays (
    session_key TEXT PRIMARY KEY REFERENCES sessions(session_key) ON DELETE CASCADE,
    display_id TEXT NOT NULL UNIQUE CHECK(length(display_id) = 64)
) STRICT;
CREATE TRIGGER display_insert AFTER INSERT ON session_displays BEGIN
    UPDATE history_usage SET metadata_bytes = metadata_bytes + 256;
END;
CREATE TRIGGER display_delete AFTER DELETE ON session_displays BEGIN
    UPDATE history_usage SET metadata_bytes = metadata_bytes - 256;
END;

CREATE TABLE journal_records (
    message_handle TEXT PRIMARY KEY CHECK(length(message_handle) = 64),
    chat_id INTEGER NOT NULL CHECK(chat_id != 0),
    message_id INTEGER NOT NULL CHECK(message_id > 0),
    direction TEXT NOT NULL CHECK(direction IN ('incoming', 'outgoing', 'service')),
    sender_user_id INTEGER CHECK(sender_user_id > 0),
    chat_label TEXT,
    sender_label TEXT,
    session_display_id TEXT,
    session_title TEXT,
    directory_label TEXT,
    body TEXT,
    message_date INTEGER NOT NULL CHECK(message_date >= 0),
    observed_at INTEGER NOT NULL CHECK(observed_at >= 0),
    expires_at INTEGER NOT NULL CHECK(expires_at >= observed_at),
    body_bytes INTEGER NOT NULL CHECK(body_bytes >= 0),
    metadata_bytes INTEGER NOT NULL CHECK(metadata_bytes >= 512),
    UNIQUE(chat_id, message_id),
    CHECK(direction != 'incoming' OR sender_user_id IS NOT NULL)
) STRICT;
CREATE INDEX journal_page ON journal_records(chat_id, observed_at, message_handle);
CREATE INDEX journal_session ON journal_records(session_display_id, chat_id, observed_at, message_handle);
CREATE INDEX journal_expiry ON journal_records(expires_at);
CREATE TRIGGER journal_insert AFTER INSERT ON journal_records BEGIN
    UPDATE history_usage SET records = records + 1,
        body_bytes = body_bytes + NEW.body_bytes, metadata_bytes = metadata_bytes + NEW.metadata_bytes;
END;
CREATE TRIGGER journal_delete AFTER DELETE ON journal_records BEGIN
    UPDATE history_usage SET records = records - 1,
        body_bytes = body_bytes - OLD.body_bytes, metadata_bytes = metadata_bytes - OLD.metadata_bytes;
END;

-- Private adapter provenance, never serialized into shared views. Either source may expire first.
CREATE TABLE attachment_records (
    attachment_handle TEXT PRIMARY KEY CHECK(length(attachment_handle) = 64),
    chat_id INTEGER NOT NULL CHECK(chat_id != 0),
    message_id INTEGER NOT NULL CHECK(message_id > 0),
    sender_user_id INTEGER CHECK(sender_user_id > 0),
    reply_id TEXT REFERENCES replies(reply_id) ON DELETE SET NULL,
    message_handle TEXT REFERENCES journal_records(message_handle) ON DELETE SET NULL,
    file_id TEXT NOT NULL CHECK(length(file_id) > 0),
    file_unique_id TEXT NOT NULL CHECK(length(file_unique_id) > 0),
    filename TEXT,
    mime_type TEXT,
    file_size INTEGER CHECK(file_size >= 0),
    expires_at INTEGER NOT NULL CHECK(expires_at >= 0),
    metadata_bytes INTEGER NOT NULL CHECK(metadata_bytes >= 512),
    kind TEXT NOT NULL CHECK(kind IN ('document', 'photo')),
    width INTEGER CHECK(width > 0),
    height INTEGER CHECK(height > 0),
    CHECK((kind = 'document' AND width IS NULL AND height IS NULL)
        OR (kind = 'photo' AND width IS NOT NULL AND height IS NOT NULL)),
    UNIQUE(chat_id, message_id, file_unique_id)
) STRICT;
CREATE INDEX attachment_expiry ON attachment_records(expires_at);
CREATE INDEX attachment_reply ON attachment_records(reply_id);
CREATE INDEX attachment_message ON attachment_records(message_handle);
CREATE INDEX attachment_orphans ON attachment_records(attachment_handle)
    WHERE reply_id IS NULL AND message_handle IS NULL;
CREATE TRIGGER attachment_insert AFTER INSERT ON attachment_records BEGIN
    UPDATE history_usage SET attachments = attachments + 1, metadata_bytes = metadata_bytes + NEW.metadata_bytes;
END;
CREATE TRIGGER attachment_delete AFTER DELETE ON attachment_records BEGIN
    UPDATE history_usage SET attachments = attachments - 1, metadata_bytes = metadata_bytes - OLD.metadata_bytes;
END;
