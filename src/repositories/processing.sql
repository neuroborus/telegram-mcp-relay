-- Keep only the latest recovery proof, atomically with its processing token rotation.
ALTER TABLE replies ADD COLUMN reconciliation_request_id TEXT
    CHECK(reconciliation_request_id IS NULL
        OR length(CAST(reconciliation_request_id AS BLOB)) BETWEEN 1 AND 64);
ALTER TABLE replies ADD COLUMN reconciliation_from_token TEXT CHECK(
    (reconciliation_request_id IS NULL AND reconciliation_from_token IS NULL)
    OR (state = 'processing' AND reconciliation_request_id IS NOT NULL
        AND reconciliation_from_token IS NOT NULL AND length(reconciliation_from_token) > 0)
);
