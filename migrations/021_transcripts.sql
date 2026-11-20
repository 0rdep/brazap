-- Local speech-to-text results for voice messages, keyed by the stored message
-- identity. Derived data: the audio and its message are never changed, and a
-- row can be recomputed. Failed rows are permanent only for audio that cannot
-- be decoded; transient failures stay in memory.
CREATE TABLE IF NOT EXISTS brazap_transcripts(
    account INTEGER NOT NULL, chat TEXT NOT NULL, sender TEXT NOT NULL, id TEXT NOT NULL,
    status TEXT NOT NULL,
    text TEXT NOT NULL DEFAULT '',
    language TEXT NOT NULL DEFAULT '',
    partial INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    model TEXT NOT NULL DEFAULT '',
    -- Lowercase text appended to brazap_message_search, so search finds it.
    search TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    PRIMARY KEY(account,chat,sender,id)
) WITHOUT ROWID;
INSERT OR IGNORE INTO brazap_schema(version) VALUES(21);
