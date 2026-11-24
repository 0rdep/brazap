-- Earlier versions cached a "not authorized" photo lookup (common right after a
-- new pairing, before WhatsApp's privacy tokens return) as "no photo" for a day.
-- Ask once more for every cached empty avatar; stored photos are untouched.
UPDATE brazap_assets SET expires_at=0
WHERE data IS NULL AND (key LIKE 'avatar:%' OR key LIKE 'profile:%')
AND NOT EXISTS(SELECT 1 FROM brazap_schema WHERE version=24);
INSERT OR IGNORE INTO brazap_schema(version) VALUES(24);
