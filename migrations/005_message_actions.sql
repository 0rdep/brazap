CREATE TABLE IF NOT EXISTS brazap_reactions(
    account INTEGER NOT NULL, chat TEXT NOT NULL,
    target_sender TEXT NOT NULL, target_id TEXT NOT NULL,
    reactor TEXT NOT NULL, emoji TEXT NOT NULL, timestamp_ms INTEGER NOT NULL,
    PRIMARY KEY(account,chat,target_sender,target_id,reactor)
);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(5);
