CREATE TABLE IF NOT EXISTS brazap_chat_reads(
    account INTEGER NOT NULL, chat TEXT NOT NULL,
    through_timestamp INTEGER NOT NULL DEFAULT 0,
    changed_at_ms INTEGER NOT NULL DEFAULT 0,
    manual_unread INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(account,chat)
);
CREATE TABLE IF NOT EXISTS brazap_read_messages(
    account INTEGER NOT NULL, chat TEXT NOT NULL, sender TEXT NOT NULL DEFAULT '',
    id TEXT NOT NULL, PRIMARY KEY(account,chat,sender,id)
);
CREATE TABLE IF NOT EXISTS brazap_read_sync(
    account INTEGER NOT NULL, chat TEXT NOT NULL, targets TEXT NOT NULL,
    generation INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(account,chat)
);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(16);
