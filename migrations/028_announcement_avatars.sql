-- Announcement groups show their community's picture, fetched on the
-- community route. Ask again once for those cached as "no photo" before.
UPDATE brazap_assets SET expires_at=0
WHERE data IS NULL AND account IN (SELECT account FROM brazap_group_hierarchy WHERE kind='announcement')
AND key IN (SELECT 'avatar:'||jid FROM brazap_group_hierarchy WHERE kind='announcement'
            UNION ALL SELECT 'profile:'||jid FROM brazap_group_hierarchy WHERE kind='announcement')
AND NOT EXISTS(SELECT 1 FROM brazap_schema WHERE version=28);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(28);
