CREATE TABLE IF NOT EXISTS brazap_deletions (
    account INTEGER NOT NULL, chat TEXT NOT NULL, id TEXT NOT NULL, deleted_at INTEGER NOT NULL,
    PRIMARY KEY(account,chat,id)
);
