-- Keyset pagination must bound candidate scans even when all sessions have valid children.
DROP INDEX sessions_expiry;
CREATE INDEX sessions_expiry ON sessions(expires_at, session_key);
