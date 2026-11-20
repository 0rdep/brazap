CREATE INDEX IF NOT EXISTS brazap_calls_peer_timeline ON brazap_calls(account,peer,timestamp,id);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(23);
