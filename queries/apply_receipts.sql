WITH best AS (
    SELECT status,
           CASE status WHEN 'read' THEN 4 WHEN 'delivered' THEN 3
                       WHEN 'sent' THEN 2 WHEN 'failed' THEN 1 ELSE 0 END AS rank
    FROM brazap_receipts
    WHERE account=?1 AND chat IN (SELECT value FROM json_each(?2)) AND id=?3
    ORDER BY rank DESC LIMIT 1
)
UPDATE brazap_messages
SET status=(SELECT status FROM best)
WHERE account=?1 AND chat IN (SELECT value FROM json_each(?2)) AND id=?3
  AND json_extract(payload,'$.from_me')=1
  AND (CASE status WHEN 'read' THEN 4 WHEN 'delivered' THEN 3
                   WHEN 'sent' THEN 2 WHEN 'failed' THEN 1 ELSE 0 END)
      < (SELECT rank FROM best)
