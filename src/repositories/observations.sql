-- Update IDs can be reused by a later Telegram sequence. Source identity is durable.

-- Rebuild under the protected migration transaction; copy every fencing/recovery field.

CREATE TABLE replies_new (
    reply_id TEXT PRIMARY KEY NOT NULL CHECK(length(reply_id) > 0),
    telegram_update_id INTEGER NOT NULL CHECK(telegram_update_id >= 0),
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

ALTER TABLE replies_new ADD COLUMN reconciliation_request_id TEXT
    CHECK(reconciliation_request_id IS NULL
        OR length(CAST(reconciliation_request_id AS BLOB)) BETWEEN 1 AND 64);
ALTER TABLE replies_new ADD COLUMN reconciliation_from_token TEXT CHECK(
    (reconciliation_request_id IS NULL AND reconciliation_from_token IS NULL)
    OR (state = 'processing' AND reconciliation_request_id IS NOT NULL
        AND reconciliation_from_token IS NOT NULL AND length(reconciliation_from_token) > 0)
);

INSERT INTO replies_new SELECT * FROM replies;

DROP TABLE replies;

ALTER TABLE replies_new RENAME TO replies;

CREATE TABLE acknowledgment_receipts_new (
    reply_id TEXT PRIMARY KEY NOT NULL CHECK(length(reply_id) > 0),
    session_key TEXT NOT NULL REFERENCES sessions(session_key) ON DELETE RESTRICT,
    claim_token TEXT NOT NULL CHECK(length(claim_token) > 0),
    telegram_update_id INTEGER NOT NULL CHECK(telegram_update_id >= 0),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    expires_at INTEGER NOT NULL CHECK(expires_at >= created_at)
) STRICT;

INSERT INTO acknowledgment_receipts_new SELECT * FROM acknowledgment_receipts;

DROP TABLE acknowledgment_receipts;

ALTER TABLE acknowledgment_receipts_new RENAME TO acknowledgment_receipts;

CREATE TABLE notice_attempts_new (
    telegram_update_id INTEGER NOT NULL CHECK(telegram_update_id >= 0),
    telegram_chat_id INTEGER NOT NULL CHECK(telegram_chat_id != 0),
    trigger_message_id INTEGER NOT NULL CHECK(trigger_message_id > 0),
    kind TEXT NOT NULL CHECK(kind IN ('unconfirmed', 'unroutable')),
    notice_message_id INTEGER CHECK(notice_message_id > 0),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    expires_at INTEGER NOT NULL CHECK(expires_at >= created_at),
    UNIQUE(telegram_chat_id, notice_message_id)
) STRICT;

INSERT INTO notice_attempts_new SELECT * FROM notice_attempts;

DROP TABLE notice_attempts;

ALTER TABLE notice_attempts_new RENAME TO notice_attempts;

CREATE TABLE update_dispositions_new (
    telegram_update_id INTEGER NOT NULL CHECK(telegram_update_id >= 0),
    kind TEXT NOT NULL CHECK(kind IN ('ignored', 'unroutable')),
    telegram_chat_id INTEGER,
    telegram_message_id INTEGER,
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    expires_at INTEGER NOT NULL CHECK(expires_at >= created_at),
    CHECK((kind = 'ignored' AND telegram_chat_id IS NULL AND telegram_message_id IS NULL)
        OR (kind = 'unroutable' AND telegram_chat_id IS NOT NULL AND telegram_chat_id != 0
            AND telegram_message_id IS NOT NULL AND telegram_message_id > 0))
) STRICT;

ALTER TABLE update_dispositions_new ADD COLUMN notice_attempted INTEGER NOT NULL DEFAULT 0 CHECK(notice_attempted IN (0, 1));

INSERT INTO update_dispositions_new SELECT * FROM update_dispositions;

DROP TABLE update_dispositions;

ALTER TABLE update_dispositions_new RENAME TO update_dispositions;

CREATE INDEX replies_inbox ON replies(session_key, state, created_at, reply_id);

CREATE INDEX replies_question ON replies(session_key, telegram_chat_id, reply_to_bot_message_id, state, created_at);

CREATE INDEX replies_expiry ON replies(expires_at);

CREATE INDEX replies_notice ON replies(created_at, telegram_update_id)
    WHERE processing_started_at IS NULL AND notice_attempted = 0;

CREATE INDEX replies_ordered ON replies(session_key, created_at, telegram_update_id, reply_id);

CREATE INDEX receipts_session ON acknowledgment_receipts(session_key);

CREATE INDEX receipts_expiry ON acknowledgment_receipts(expires_at);

CREATE INDEX notices_expiry ON notice_attempts(expires_at);

-- Keep even multiple legacy attempts for one source; each may own a confirmed notice ID.
CREATE UNIQUE INDEX notices_trigger ON notice_attempts(telegram_chat_id, trigger_message_id, telegram_update_id);

CREATE INDEX dispositions_expiry ON update_dispositions(expires_at);

CREATE UNIQUE INDEX dispositions_ignored ON update_dispositions(kind) WHERE kind = 'ignored';

CREATE UNIQUE INDEX dispositions_message ON update_dispositions(telegram_chat_id, telegram_message_id, telegram_update_id)
    WHERE kind = 'unroutable';

CREATE INDEX dispositions_notice ON update_dispositions(created_at, telegram_update_id)
    WHERE kind = 'unroutable' AND notice_attempted = 0;

ALTER TABLE acknowledgment_receipts ADD COLUMN telegram_chat_id INTEGER;

ALTER TABLE acknowledgment_receipts ADD COLUMN telegram_message_id INTEGER CHECK(
    (telegram_chat_id IS NULL AND telegram_message_id IS NULL)
    OR (telegram_chat_id IS NOT NULL AND telegram_chat_id != 0
        AND telegram_message_id IS NOT NULL AND telegram_message_id > 0));

CREATE INDEX receipts_message ON acknowledgment_receipts(telegram_chat_id, telegram_message_id);

ALTER TABLE journal_records ADD COLUMN reply_to_message_id INTEGER CHECK(reply_to_message_id > 0);

ALTER TABLE journal_records ADD COLUMN telegram_update_id INTEGER CHECK(telegram_update_id >= 0);

ALTER TABLE telegram_cursor ADD COLUMN telegram_chat_id INTEGER;

ALTER TABLE telegram_cursor ADD COLUMN telegram_message_id INTEGER CHECK(
    (telegram_chat_id IS NULL AND telegram_message_id IS NULL)
    OR (telegram_chat_id IS NOT NULL AND telegram_chat_id != 0
        AND telegram_message_id IS NOT NULL AND telegram_message_id > 0));
