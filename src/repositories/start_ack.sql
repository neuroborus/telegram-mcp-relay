-- Existing processing replies have already started; leave both fields null.
ALTER TABLE replies ADD COLUMN start_ack_state TEXT
    CHECK(start_ack_state IN ('attempted', 'sent'));
ALTER TABLE replies ADD COLUMN start_ack_text TEXT CHECK(
    (start_ack_state IS NULL AND start_ack_text IS NULL)
    OR (state = 'processing' AND start_ack_state IS NOT NULL AND start_ack_text IS NOT NULL
        AND length(CAST(start_ack_text AS BLOB)) <= 4000)
);
