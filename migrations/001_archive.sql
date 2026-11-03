CREATE TABLE IF NOT EXISTS brazap_schema(version INTEGER PRIMARY KEY);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(1);
CREATE TABLE IF NOT EXISTS brazap_chats(
    account INTEGER NOT NULL, jid TEXT NOT NULL, name TEXT NOT NULL DEFAULT '',
    preview TEXT NOT NULL DEFAULT '', updated_at INTEGER NOT NULL DEFAULT 0,
    unread INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(account,jid)
);
CREATE TABLE IF NOT EXISTS brazap_contacts(
    account INTEGER NOT NULL, jid TEXT NOT NULL, name TEXT NOT NULL,
    payload TEXT NOT NULL, PRIMARY KEY(account,jid)
);
CREATE TABLE IF NOT EXISTS brazap_messages(
    account INTEGER NOT NULL, chat TEXT NOT NULL, sender TEXT NOT NULL, id TEXT NOT NULL,
    timestamp INTEGER NOT NULL, status TEXT NOT NULL, payload TEXT NOT NULL,
    encoded BLOB NOT NULL, metadata TEXT NOT NULL,
    PRIMARY KEY(account,chat,sender,id)
);
CREATE INDEX IF NOT EXISTS brazap_messages_timeline ON brazap_messages(account,chat,timestamp DESC,id DESC);
CREATE TABLE IF NOT EXISTS brazap_media(
    account INTEGER NOT NULL, chat TEXT NOT NULL, sender TEXT NOT NULL,
    id TEXT NOT NULL, data BLOB NOT NULL, PRIMARY KEY(account,chat,sender,id)
);
CREATE TABLE IF NOT EXISTS brazap_history(
    account INTEGER NOT NULL, chat TEXT NOT NULL, sender TEXT NOT NULL, id TEXT NOT NULL,
    stage INTEGER NOT NULL, data BLOB NOT NULL, metadata TEXT NOT NULL,
    processed INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(account,chat,sender,id,stage)
);
CREATE TABLE IF NOT EXISTS brazap_events(
    account INTEGER NOT NULL, kind TEXT NOT NULL, identity TEXT NOT NULL,
    payload TEXT NOT NULL, PRIMARY KEY(account,kind,identity)
);
