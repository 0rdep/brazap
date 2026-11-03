CREATE TABLE IF NOT EXISTS brazap_assets(
    account INTEGER NOT NULL, key TEXT NOT NULL, mime TEXT NOT NULL,
    data BLOB, updated_at INTEGER NOT NULL, expires_at INTEGER NOT NULL,
    PRIMARY KEY(account,key)
);
CREATE TABLE IF NOT EXISTS brazap_calls(
    account INTEGER NOT NULL, creator TEXT NOT NULL, id TEXT NOT NULL,
    peer TEXT NOT NULL, timestamp INTEGER, payload TEXT NOT NULL, record BLOB NOT NULL,
    PRIMARY KEY(account,creator,id)
);
CREATE INDEX IF NOT EXISTS brazap_calls_timeline ON brazap_calls(account,timestamp DESC);
CREATE TABLE IF NOT EXISTS brazap_preferences(
    account INTEGER NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY(account,key)
);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(2);
