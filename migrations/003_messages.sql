CREATE TABLE IF NOT EXISTS brazap_message_search (
    account INTEGER NOT NULL, chat TEXT NOT NULL, sender TEXT NOT NULL,
    id TEXT NOT NULL, text TEXT NOT NULL,
    PRIMARY KEY(account,chat,sender,id)
);
CREATE TABLE IF NOT EXISTS brazap_receipts (
    account INTEGER NOT NULL, chat TEXT NOT NULL, id TEXT NOT NULL,
    status TEXT NOT NULL,
    PRIMARY KEY(account,chat,id)
);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(3);
