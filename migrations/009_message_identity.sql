-- Alias lookups and duplicate-message checks must not scan message bodies.
CREATE INDEX IF NOT EXISTS brazap_messages_identity ON brazap_messages(account,chat,id);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(9);
