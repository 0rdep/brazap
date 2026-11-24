-- Where each group sits in a community, as WhatsApp reported it at the last
-- connection: 'community' (a community parent), 'subgroup' or 'announcement'
-- (the community's announcement group; parent is set for both), or 'group'. Derived data; rebuilt from the server whenever Brazap connects.
CREATE TABLE IF NOT EXISTS brazap_group_hierarchy(
    account INTEGER NOT NULL, jid TEXT NOT NULL, kind TEXT NOT NULL,
    parent TEXT NOT NULL DEFAULT '', subject TEXT NOT NULL DEFAULT '',
    PRIMARY KEY(account,jid)
);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(26);
