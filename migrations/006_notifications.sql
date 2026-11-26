CREATE TABLE IF NOT EXISTS brazap_notification_deliveries(
    account INTEGER NOT NULL,
    chat TEXT NOT NULL,
    sender TEXT NOT NULL,
    id TEXT NOT NULL,
    PRIMARY KEY(account,chat,sender,id)
);
-- Archive::open seeds recent deliveries after 017 builds the index it needs.
INSERT OR IGNORE INTO brazap_schema(version) VALUES(6);
