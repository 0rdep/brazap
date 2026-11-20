-- The latest edit of each message. Derived data: every MESSAGE_EDIT protocol
-- message stays in brazap_messages with its raw protobuf, and reads overlay
-- this row on the original, whose payload and encoded bytes never change.
-- Rows may precede their original (out-of-order delivery or history sync).
CREATE TABLE IF NOT EXISTS brazap_edits(
    account INTEGER NOT NULL, chat TEXT NOT NULL, sender TEXT NOT NULL, id TEXT NOT NULL,
    -- The edit's own message id, so a replay of the same edit is idempotent.
    edit_id TEXT NOT NULL,
    -- When the edit was made, in Unix seconds.
    edited_at INTEGER NOT NULL,
    -- The new body or caption and its mentions and link preview, as JSON.
    content TEXT NOT NULL,
    -- The chat-list preview of the edited message, once the original is known.
    preview TEXT NOT NULL DEFAULT '',
    PRIMARY KEY(account,chat,sender,id)
) WITHOUT ROWID;
INSERT OR IGNORE INTO brazap_schema(version) VALUES(22);
