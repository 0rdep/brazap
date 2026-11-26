-- Contact address aliases belong to the retained archive, independently of
-- the SDK authentication tables that are cleared when unlinking a device.
CREATE TABLE IF NOT EXISTS brazap_identity_links (
    account INTEGER NOT NULL,
    lid TEXT NOT NULL,
    phone_number TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(account,lid)
);
INSERT INTO brazap_identity_links(account,lid,phone_number,updated_at)
SELECT device_id,lid,phone_number,updated_at FROM lid_pn_mapping WHERE true
ON CONFLICT(account,lid) DO UPDATE SET phone_number=excluded.phone_number,updated_at=excluded.updated_at
-- Runs on every open: skip rows that would be rewritten unchanged.
WHERE excluded.updated_at>brazap_identity_links.updated_at
   OR (excluded.updated_at=brazap_identity_links.updated_at
       AND excluded.phone_number IS NOT brazap_identity_links.phone_number);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(12);
