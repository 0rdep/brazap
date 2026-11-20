WITH aliases AS (SELECT value FROM json_each(?1)),
     calls AS (
         SELECT c.id,c.creator,c.peer,c.timestamp,c.payload,
                CASE json_extract(c.payload,'$.from_me')
                    WHEN 1 THEN 'self'
                    WHEN 0 THEN COALESCE(NULLIF(c.creator,''),c.peer)
                    ELSE ''
                END AS sender,
                ROW_NUMBER() OVER (
                    PARTITION BY c.id
                    ORDER BY (COALESCE(json_extract(c.payload,'$.duration'),0) > 0) DESC,
                             (json_extract(c.payload,'$.outcome') != 'unknown') DESC,
                             c.creator
                ) AS representative
         FROM brazap_calls c
         WHERE c.account=?2 AND c.peer IN (SELECT value FROM aliases)
           AND c.timestamp>COALESCE((
               SELECT MAX(d.through_timestamp) FROM brazap_deleted_chats d
               WHERE d.account=c.account AND d.jid IN (SELECT value FROM aliases)
           ),0)
           AND c.timestamp {timestamp_bound} ?3
     )
SELECT c.timestamp,c.sender,c.payload
FROM calls c
WHERE c.representative=1
  AND (c.timestamp,'call-log:' || c.id,c.sender) {cursor_bound} (?3,?4,?5)
  AND NOT EXISTS (
      SELECT 1 FROM brazap_messages m
      WHERE m.account=?2 AND m.chat IN (SELECT value FROM aliases)
        AND m.timestamp=c.timestamp
        AND json_extract(m.payload,'$.kind') IN (
            'call',CASE json_extract(c.payload,'$.video') WHEN 1 THEN 'video_call' ELSE 'voice_call' END
        )
        AND json_extract(m.payload,'$.from_me')=json_extract(c.payload,'$.from_me')
        AND (m.chat NOT LIKE '%@g.us' OR m.sender=c.sender)
  )
ORDER BY c.timestamp {order}, c.id {order}, c.sender {order}
LIMIT ?6
