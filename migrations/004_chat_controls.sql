CREATE TABLE IF NOT EXISTS brazap_chat_settings(
    account INTEGER NOT NULL, jid TEXT NOT NULL,
    muted INTEGER NOT NULL DEFAULT 0, mute_until_ms INTEGER NOT NULL DEFAULT 0,
    pinned INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0,
    mute_changed_at INTEGER NOT NULL DEFAULT 0,
    pin_changed_at INTEGER NOT NULL DEFAULT 0,
    archive_changed_at INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(account,jid)
);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(4);
