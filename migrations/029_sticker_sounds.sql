-- The sound of a sticker in My stickers, kept to upload with it each time it is
-- sent (WhatsApp's audio sticker). Ogg Opus bytes and their length in seconds.
CREATE TABLE IF NOT EXISTS brazap_sticker_sounds(
    account INTEGER NOT NULL, hash TEXT NOT NULL, audio BLOB NOT NULL,
    seconds INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(account,hash)
);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(29);
