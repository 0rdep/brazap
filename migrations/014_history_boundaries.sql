-- Keep every requested boundary, including pages preceding the current cursor.
CREATE TABLE IF NOT EXISTS brazap_history_boundaries(
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
    PRIMARY KEY(account,chat,anchor_id,anchor_sender,anchor_timestamp)
);
CREATE INDEX IF NOT EXISTS brazap_history_boundary_request
    ON brazap_history_boundaries(account,request_id);
INSERT OR IGNORE INTO brazap_history_boundaries
SELECT * FROM brazap_history_requests;
INSERT OR IGNORE INTO brazap_schema(version) VALUES(14);
