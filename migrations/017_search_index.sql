-- Index substrings without duplicating the archive's text or media. Triggers
-- keep live messages, history imports, projection repairs and deletions in sync.
BEGIN IMMEDIATE;
CREATE VIRTUAL TABLE IF NOT EXISTS brazap_message_fts USING fts5(
    text, content='brazap_message_search', content_rowid='rowid',
    tokenize='trigram case_sensitive 1'
);
CREATE TRIGGER IF NOT EXISTS brazap_search_insert AFTER INSERT ON brazap_message_search BEGIN
    INSERT INTO brazap_message_fts(rowid,text) VALUES(new.rowid,new.text);
END;
CREATE TRIGGER IF NOT EXISTS brazap_search_delete AFTER DELETE ON brazap_message_search BEGIN
    INSERT INTO brazap_message_fts(brazap_message_fts,rowid,text) VALUES('delete',old.rowid,old.text);
END;
CREATE TRIGGER IF NOT EXISTS brazap_search_update AFTER UPDATE OF text ON brazap_message_search
WHEN old.text != new.text BEGIN
    INSERT INTO brazap_message_fts(brazap_message_fts,rowid,text) VALUES('delete',old.rowid,old.text);
    INSERT INTO brazap_message_fts(rowid,text) VALUES(new.rowid,new.text);
END;
CREATE INDEX IF NOT EXISTS brazap_messages_search_order
    ON brazap_messages(account,timestamp DESC,id DESC,sender DESC);
INSERT INTO brazap_message_fts(brazap_message_fts)
    SELECT 'rebuild' WHERE NOT EXISTS(SELECT 1 FROM brazap_schema WHERE version=17);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(17);
COMMIT;
