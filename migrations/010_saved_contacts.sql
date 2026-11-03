CREATE TABLE IF NOT EXISTS brazap_saved_contacts (
    account INTEGER NOT NULL,
    jid TEXT NOT NULL,
    name TEXT NOT NULL,
    payload TEXT NOT NULL,
    PRIMARY KEY(account,jid)
);
-- A push name can overwrite the old mixed contact cache. Recover address-book
-- names from the retained contact-sync events, then keep the two sources apart.
INSERT OR REPLACE INTO brazap_saved_contacts(account,jid,name,payload)
SELECT account,jid,COALESCE(NULLIF(trim(json_extract(payload,'$.action.full_name')),''),trim(json_extract(payload,'$.action.first_name')),''),payload
FROM brazap_contacts WHERE json_type(payload,'$.action')='object'
AND NOT EXISTS(SELECT 1 FROM brazap_schema WHERE version=10);
INSERT OR REPLACE INTO brazap_saved_contacts(account,jid,name,payload)
SELECT e.account,json_extract(e.payload,'$.ContactUpdate.jid'),
       COALESCE(NULLIF(trim(json_extract(e.payload,'$.ContactUpdate.action.full_name')),''),trim(json_extract(e.payload,'$.ContactUpdate.action.first_name')),''),
       json_extract(e.payload,'$.ContactUpdate')
FROM brazap_events e
WHERE e.kind='ContactUpdate' AND json_extract(e.payload,'$.ContactUpdate.jid') IS NOT NULL
AND e.rowid=(SELECT MAX(newer.rowid) FROM brazap_events newer WHERE newer.account=e.account AND newer.kind='ContactUpdate' AND json_extract(newer.payload,'$.ContactUpdate.jid')=json_extract(e.payload,'$.ContactUpdate.jid'))
AND NOT EXISTS(SELECT 1 FROM brazap_schema WHERE version=10);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(10);
