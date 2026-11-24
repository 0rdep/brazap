-- Businesses often send no push name, only the verified name certificate kept
-- in each message's envelope. Name earlier private chats from it once; saved
-- contact names still take precedence when chats are listed.
UPDATE brazap_chats SET name=(
    SELECT trim(json_extract(m.metadata,'$.verified_name.name')) FROM brazap_messages m
    WHERE m.account=brazap_chats.account AND m.chat=brazap_chats.jid AND m.sender!='self'
      AND trim(COALESCE(json_extract(m.metadata,'$.verified_name.name'),''))!=''
    ORDER BY m.timestamp DESC LIMIT 1)
WHERE trim(name)='' AND jid NOT LIKE '%@g.us' AND jid NOT LIKE '%@newsletter' AND jid NOT LIKE '%@broadcast'
AND NOT EXISTS(SELECT 1 FROM brazap_schema WHERE version=25)
AND EXISTS(
    SELECT 1 FROM brazap_messages m
    WHERE m.account=brazap_chats.account AND m.chat=brazap_chats.jid AND m.sender!='self'
      AND trim(COALESCE(json_extract(m.metadata,'$.verified_name.name'),''))!='');
INSERT OR IGNORE INTO brazap_schema(version) VALUES(25);
