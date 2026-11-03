CREATE TABLE IF NOT EXISTS brazap_history_requests(
    account INTEGER NOT NULL,
    chat TEXT NOT NULL,
    request_id TEXT NOT NULL,
    anchor_id TEXT NOT NULL,
    anchor_sender TEXT NOT NULL,
    anchor_timestamp INTEGER NOT NULL,
    requested_at INTEGER NOT NULL,
    response_at INTEGER NOT NULL DEFAULT 0,
    complete INTEGER NOT NULL DEFAULT 0,
    next_auto_at INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(account,chat)
);
CREATE INDEX IF NOT EXISTS brazap_history_request_id
    ON brazap_history_requests(account,request_id);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(8);
