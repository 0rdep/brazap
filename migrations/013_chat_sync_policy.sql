CREATE TABLE IF NOT EXISTS brazap_chat_locks(
    account INTEGER NOT NULL, jid TEXT NOT NULL, locked INTEGER NOT NULL,
    changed_at INTEGER NOT NULL, PRIMARY KEY(account,jid)
);
CREATE TABLE IF NOT EXISTS brazap_history_complete(
    account INTEGER NOT NULL, chat TEXT NOT NULL, completed_at INTEGER NOT NULL,
    reason TEXT NOT NULL, PRIMARY KEY(account,chat)
);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(13);
CREATE TABLE IF NOT EXISTS brazap_history_prefetched(
    account INTEGER NOT NULL, chat TEXT NOT NULL, completed_at INTEGER NOT NULL,
    PRIMARY KEY(account,chat)
);
-- Existing completed requests already satisfy the one-time background warmup.
INSERT OR IGNORE INTO brazap_history_prefetched(account,chat,completed_at)
SELECT account,chat,MAX(response_at,requested_at) FROM brazap_history_requests WHERE complete=1;
