-- Optional bounded display metadata; existing identities and routes remain unchanged.
ALTER TABLE sessions ADD COLUMN directory_label TEXT
    CHECK(directory_label IS NULL OR length(directory_label) BETWEEN 1 AND 100);
