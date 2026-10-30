-- Keep replies from one ordered Telegram batch in arrival order at equal receipt times.
CREATE INDEX replies_ordered ON replies(session_key, created_at, telegram_update_id);
