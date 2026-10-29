-- Unix timestamps are integer milliseconds. Retention and admission are service policies.
CREATE TABLE sessions (
    session_key TEXT PRIMARY KEY NOT NULL CHECK(length(session_key) > 0),
    external_session_id TEXT UNIQUE CHECK(external_session_id IS NULL OR length(external_session_id) > 0),
    title TEXT NOT NULL CHECK(length(title) > 0),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    last_activity_at INTEGER NOT NULL CHECK(last_activity_at >= created_at),
    expires_at INTEGER NOT NULL CHECK(expires_at >= last_activity_at)
) STRICT;
CREATE INDEX sessions_expiry ON sessions(expires_at);

CREATE TABLE message_routes (
    telegram_chat_id INTEGER NOT NULL CHECK(telegram_chat_id != 0),
    telegram_message_id INTEGER NOT NULL CHECK(telegram_message_id > 0),
    session_key TEXT NOT NULL REFERENCES sessions(session_key) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK(kind IN ('text', 'document')),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    expires_at INTEGER NOT NULL CHECK(expires_at >= created_at),
    PRIMARY KEY(telegram_chat_id, telegram_message_id)
) STRICT, WITHOUT ROWID;
CREATE INDEX routes_session ON message_routes(session_key);
CREATE INDEX routes_expiry ON message_routes(expires_at);

CREATE TABLE replies (
    reply_id TEXT PRIMARY KEY NOT NULL CHECK(length(reply_id) > 0),
    telegram_update_id INTEGER NOT NULL UNIQUE CHECK(telegram_update_id >= 0),
    telegram_chat_id INTEGER NOT NULL CHECK(telegram_chat_id != 0),
    telegram_message_id INTEGER NOT NULL CHECK(telegram_message_id > 0),
    reply_to_bot_message_id INTEGER NOT NULL CHECK(reply_to_bot_message_id > 0),
    session_key TEXT NOT NULL REFERENCES sessions(session_key) ON DELETE RESTRICT,
    text_or_caption TEXT,
    attachment_metadata_json TEXT CHECK(attachment_metadata_json IS NULL OR json_valid(attachment_metadata_json)),
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending', 'claimed', 'processing')),
    claimed_at INTEGER CHECK(claimed_at >= 0),
    claim_token TEXT CHECK(claim_token IS NULL OR length(claim_token) > 0),
    claim_expires_at INTEGER,
    processing_started_at INTEGER CHECK(processing_started_at >= 0),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    expires_at INTEGER NOT NULL CHECK(expires_at >= created_at),
    -- This records an attempt decision, not proof that Telegram accepted a notice.
    notice_attempted INTEGER NOT NULL DEFAULT 0 CHECK(notice_attempted IN (0, 1)),
    UNIQUE(telegram_chat_id, telegram_message_id),
    CHECK(
        (state = 'pending' AND claimed_at IS NULL AND claim_token IS NULL
            AND claim_expires_at IS NULL AND processing_started_at IS NULL)
        OR (state = 'claimed' AND claimed_at IS NOT NULL AND claim_token IS NOT NULL
            AND claim_expires_at IS NOT NULL AND claim_expires_at >= claimed_at
            AND processing_started_at IS NULL)
        OR (state = 'processing' AND claimed_at IS NOT NULL AND claim_token IS NOT NULL
            AND claim_expires_at IS NULL AND processing_started_at IS NOT NULL)
    )
) STRICT;
CREATE INDEX replies_inbox ON replies(session_key, state, created_at, reply_id);
CREATE INDEX replies_question ON replies(session_key, telegram_chat_id, reply_to_bot_message_id, state, created_at);
CREATE INDEX replies_expiry ON replies(expires_at);
CREATE INDEX replies_notice ON replies(created_at) WHERE processing_started_at IS NULL AND notice_attempted = 0;

CREATE TABLE telegram_cursor (
    singleton_id INTEGER PRIMARY KEY CHECK(singleton_id = 1),
    last_durably_processed_update_id INTEGER CHECK(last_durably_processed_update_id >= 0)
) STRICT;
INSERT INTO telegram_cursor(singleton_id) VALUES(1);

-- No reply FK: completed payloads can be removed while valid-token receipts survive.
CREATE TABLE acknowledgment_receipts (
    reply_id TEXT PRIMARY KEY NOT NULL CHECK(length(reply_id) > 0),
    session_key TEXT NOT NULL REFERENCES sessions(session_key) ON DELETE RESTRICT,
    claim_token TEXT NOT NULL CHECK(length(claim_token) > 0),
    telegram_update_id INTEGER NOT NULL UNIQUE CHECK(telegram_update_id >= 0),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    expires_at INTEGER NOT NULL CHECK(expires_at >= created_at)
) STRICT;
CREATE INDEX receipts_session ON acknowledgment_receipts(session_key);
CREATE INDEX receipts_expiry ON acknowledgment_receipts(expires_at);

-- Persist before sending; ambiguous delivery must not authorize another attempt.
CREATE TABLE notice_attempts (
    telegram_update_id INTEGER PRIMARY KEY CHECK(telegram_update_id >= 0),
    telegram_chat_id INTEGER NOT NULL CHECK(telegram_chat_id != 0),
    trigger_message_id INTEGER NOT NULL CHECK(trigger_message_id > 0),
    kind TEXT NOT NULL CHECK(kind IN ('unconfirmed', 'unroutable')),
    notice_message_id INTEGER CHECK(notice_message_id > 0),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    expires_at INTEGER NOT NULL CHECK(expires_at >= created_at),
    UNIQUE(telegram_chat_id, notice_message_id)
) STRICT;
CREATE INDEX notices_expiry ON notice_attempts(expires_at);
