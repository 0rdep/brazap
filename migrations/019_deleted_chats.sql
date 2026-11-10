CREATE TABLE IF NOT EXISTS brazap_deleted_chats (
    account INTEGER NOT NULL, jid TEXT NOT NULL, through_timestamp INTEGER NOT NULL,
    PRIMARY KEY(account,jid)
);
CREATE TABLE IF NOT EXISTS brazap_deleted_message_ids (
    account INTEGER NOT NULL, chat TEXT NOT NULL, sender TEXT NOT NULL, id TEXT NOT NULL,
    PRIMARY KEY(account,chat,sender,id)
);
