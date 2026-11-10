-- When each retained attachment was stored and last opened. Kept apart from
-- brazap_media so existing databases need no table rewrite and this script is
-- safe to run on every start.
CREATE TABLE IF NOT EXISTS brazap_media_usage(
    account INTEGER NOT NULL, chat TEXT NOT NULL, sender TEXT NOT NULL, id TEXT NOT NULL,
    stored_at INTEGER NOT NULL, accessed_at INTEGER NOT NULL,
    PRIMARY KEY(account,chat,sender,id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS brazap_media_usage_access ON brazap_media_usage(account,accessed_at);
-- Media that already exists starts its retention clock now, so installing this
-- version never removes anything immediately.
INSERT OR IGNORE INTO brazap_media_usage(account,chat,sender,id,stored_at,accessed_at)
    SELECT account,chat,sender,id,CAST(strftime('%s','now') AS INTEGER)*1000,CAST(strftime('%s','now') AS INTEGER)*1000
    FROM brazap_media;
INSERT OR IGNORE INTO brazap_schema(version) VALUES(20);
