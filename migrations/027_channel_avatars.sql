-- Channel pictures were looked up like contact photos, which channels never
-- answer, so each was cached as "no photo". Ask again once, from the channel's
-- metadata.
UPDATE brazap_assets SET expires_at=0
WHERE data IS NULL AND (key LIKE 'avatar:%@newsletter' OR key LIKE 'profile:%@newsletter')
AND NOT EXISTS(SELECT 1 FROM brazap_schema WHERE version=27);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(27);
