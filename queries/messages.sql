SELECT m.payload,m.status,
       (SELECT d.deleted_at FROM brazap_deletions d WHERE d.account=m.account AND d.chat=m.chat AND d.id=m.id) AS deleted_at,
       COALESCE((SELECT json_extract(value,'$.privacy.retain_deleted') FROM brazap_preferences WHERE account=m.account AND key='app_settings'),1) AS retain_deleted,
       COALESCE(sender_photo.updated_at,0) AS sender_photo_version,
       COALESCE((SELECT json_group_array(json_object('sender',r.reactor,'emoji',r.emoji))
                 FROM brazap_reactions r WHERE r.account=m.account AND r.chat=m.chat
                 AND r.target_sender=m.sender AND r.target_id=m.id AND r.emoji!=''),'[]') AS reactions,
       EXISTS(SELECT 1 FROM brazap_media b
              WHERE b.account=m.account AND b.chat=m.chat AND b.sender=m.sender AND b.id=m.id) AS has_media,
       COALESCE(CASE WHEN lower(trim(sn.name)) NOT IN ('unknown contact','unknown','unnamed group','contato desconhecido','contacto desconocido') THEN NULLIF(sn.name,'') END,CASE WHEN lower(trim(sp.name)) NOT IN ('unknown contact','unknown','unnamed group','contato desconhecido','contacto desconocido') THEN NULLIF(sp.name,'') END,CASE WHEN lower(trim(sa.name)) NOT IN ('unknown contact','unknown','unnamed group','contato desconhecido','contacto desconocido') THEN NULLIF(sa.name,'') END,CASE WHEN lower(trim(n.name)) NOT IN ('unknown contact','unknown','unnamed group','contato desconhecido','contacto desconocido') THEN NULLIF(n.name,'') END,CASE WHEN lower(trim(p.name)) NOT IN ('unknown contact','unknown','unnamed group','contato desconhecido','contacto desconocido') THEN NULLIF(p.name,'') END,CASE WHEN lower(trim(a.name)) NOT IN ('unknown contact','unknown','unnamed group','contato desconhecido','contacto desconocido') THEN NULLIF(a.name,'') END,CASE WHEN lower(trim(c.name)) NOT IN ('unknown contact','unknown','unnamed group','contato desconhecido','contacto desconocido') THEN NULLIF(c.name,'') END,
                NULLIF(json_extract(m.metadata,'$.push_name'),''),
                NULLIF(json_extract(m.metadata,'$.pushName'),''),'') AS sender_name,
       COALESCE(l.phone_number,
                CASE WHEN json_extract(m.metadata,'$.source.sender_alt') LIKE '%@s.whatsapp.net'
                     THEN json_extract(m.metadata,'$.source.sender_alt') END,'') AS sender_phone
FROM brazap_messages m
-- Group senders normally use LIDs. Resolve them without treating opaque IDs as phone numbers.
-- For phone-addressed messages, use the newest reverse mapping, as the SDK does.
LEFT JOIN lid_pn_mapping l ON l.device_id=m.account AND l.lid=CASE
    WHEN m.sender LIKE '%@lid' THEN substr(m.sender,1,instr(m.sender,'@')-1)
    WHEN m.sender LIKE '%@s.whatsapp.net' THEN (
        SELECT reverse.lid FROM lid_pn_mapping reverse
        WHERE reverse.device_id=m.account
          AND reverse.phone_number=substr(m.sender,1,instr(m.sender,'@')-1)
        ORDER BY reverse.updated_at DESC LIMIT 1
    ) END
LEFT JOIN brazap_contacts n ON n.account=m.account AND n.jid=m.sender
LEFT JOIN brazap_contacts p ON p.account=m.account AND p.jid=CASE
    WHEN m.sender LIKE '%@lid' THEN l.phone_number || '@s.whatsapp.net'
    ELSE l.lid || '@lid' END
LEFT JOIN brazap_contacts a ON a.account=m.account
    AND a.jid=json_extract(m.metadata,'$.source.sender_alt')
LEFT JOIN brazap_chats c ON c.account=m.account AND c.jid=m.sender
LEFT JOIN brazap_assets sender_photo ON sender_photo.account=m.account
    AND sender_photo.key='avatar:' || m.sender

LEFT JOIN brazap_saved_contacts sn ON sn.account=m.account AND sn.jid=m.sender
LEFT JOIN brazap_saved_contacts sp ON sp.account=m.account AND sp.jid=CASE
    WHEN m.sender LIKE '%@lid' THEN l.phone_number || '@s.whatsapp.net'
    ELSE l.lid || '@lid' END
LEFT JOIN brazap_saved_contacts sa ON sa.account=m.account AND sa.jid=json_extract(m.metadata,'$.source.sender_alt')
