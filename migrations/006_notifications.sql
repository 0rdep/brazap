CREATE TABLE IF NOT EXISTS brazap_notification_deliveries(
    account INTEGER NOT NULL,
    chat TEXT NOT NULL,
    sender TEXT NOT NULL,
    id TEXT NOT NULL,
    PRIMARY KEY(account,chat,sender,id)
);
-- Existing archived messages must not produce notifications on a new launch.
INSERT OR IGNORE INTO brazap_notification_deliveries(account,chat,sender,id)
SELECT account,chat,sender,id FROM brazap_messages
WHERE timestamp>=CAST(strftime('%s','now') AS INTEGER)-120;
INSERT OR IGNORE INTO brazap_schema(version) VALUES(6);
