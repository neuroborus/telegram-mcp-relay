-- Minimal durable classifications; no unauthorized identities or message contents.
CREATE TABLE update_dispositions (
    telegram_update_id INTEGER PRIMARY KEY CHECK(telegram_update_id >= 0),
    kind TEXT NOT NULL CHECK(kind IN ('ignored', 'unroutable')),
    telegram_chat_id INTEGER,
    telegram_message_id INTEGER,
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    expires_at INTEGER NOT NULL CHECK(expires_at >= created_at),
    CHECK((kind = 'ignored' AND telegram_chat_id IS NULL AND telegram_message_id IS NULL)
        OR (kind = 'unroutable' AND telegram_chat_id IS NOT NULL AND telegram_chat_id != 0
            AND telegram_message_id IS NOT NULL AND telegram_message_id > 0))
) STRICT;
CREATE INDEX dispositions_expiry ON update_dispositions(expires_at);
-- Ignored input has no recoverable payload or notice obligation; retain one compact marker.
CREATE UNIQUE INDEX dispositions_ignored ON update_dispositions(kind) WHERE kind = 'ignored';
