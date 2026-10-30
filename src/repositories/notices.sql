ALTER TABLE update_dispositions ADD COLUMN notice_attempted INTEGER NOT NULL DEFAULT 0
    CHECK(notice_attempted IN (0, 1));
CREATE INDEX dispositions_message ON update_dispositions(telegram_chat_id, telegram_message_id)
    WHERE kind = 'unroutable';
CREATE INDEX dispositions_notice ON update_dispositions(created_at, telegram_update_id)
    WHERE kind = 'unroutable' AND notice_attempted = 0;
CREATE INDEX notices_trigger ON notice_attempts(telegram_chat_id, trigger_message_id);
DROP INDEX replies_notice;
CREATE INDEX replies_notice ON replies(created_at, telegram_update_id)
    WHERE processing_started_at IS NULL AND notice_attempted = 0;
-- Honor any attempts retained by older versions, including uncertain delivery.
UPDATE update_dispositions SET notice_attempted = 1 WHERE kind = 'unroutable' AND EXISTS(
    SELECT 1 FROM notice_attempts WHERE telegram_chat_id = update_dispositions.telegram_chat_id
        AND trigger_message_id = update_dispositions.telegram_message_id);
