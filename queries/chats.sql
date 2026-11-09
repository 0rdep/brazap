SELECT c.jid,'[]' AS aliases,
       COALESCE(CASE WHEN lower(trim(n.name)) NOT IN ('unknown contact','unknown','unnamed group','contato desconhecido','contacto desconocido') THEN NULLIF(n.name,'') END,CASE WHEN lower(trim(p.name)) NOT IN ('unknown contact','unknown','unnamed group','contato desconhecido','contacto desconocido') THEN NULLIF(p.name,'') END,CASE WHEN lower(trim(c.name)) NOT IN ('unknown contact','unknown','unnamed group','contato desconhecido','contacto desconocido') THEN NULLIF(c.name,'') END,'') AS name,
       COALESCE(l.phone_number,'') AS phone,
       COALESCE(json_extract(m.payload,'$.preview'),NULLIF(json_extract(m.payload,'$.text'),''),json_extract(m.payload,'$.file_name'),c.preview) AS preview,
       COALESCE(m.timestamp,c.updated_at) AS updated_at,c.unread,
       COALESCE(a.updated_at,0) AS photo_version,
       COALESCE(json_extract(m.payload,'$.kind'),'') AS preview_kind,
       COALESCE(m.status,'') AS preview_status,
       COALESCE(json_extract(m.payload,'$.from_me'),0) AS preview_from_me,
       COALESCE(s.pinned,0) AS pinned,COALESCE(s.archived,0) AS archived,
       CASE WHEN s.muted=1 AND (s.mute_until_ms=-1 OR s.mute_until_ms>CAST(strftime('%s','now') AS INTEGER)*1000) THEN 1 ELSE 0 END AS muted
FROM brazap_chats c
LEFT JOIN brazap_chat_settings s ON s.account=c.account AND s.jid=c.jid
LEFT JOIN lid_pn_mapping l ON l.device_id=c.account AND c.jid LIKE '%@lid'
    AND (l.lid=c.jid OR l.lid=substr(c.jid,1,instr(c.jid,'@')-1))
LEFT JOIN brazap_contacts n ON n.account=c.account AND n.jid=c.jid
LEFT JOIN brazap_contacts p ON p.account=c.account AND p.jid=l.phone_number || '@s.whatsapp.net'
LEFT JOIN brazap_assets a ON a.account=c.account AND a.key='avatar:' || c.jid
LEFT JOIN brazap_messages m ON m.rowid=(
    -- The global search-order index can otherwise win here and scan the whole
    -- account for every chat. Keep this lookup bounded to one chat's timeline.
    SELECT latest.rowid FROM brazap_messages latest INDEXED BY brazap_messages_timeline
    WHERE latest.account=c.account AND latest.chat=c.jid
      AND json_extract(latest.payload,'$.kind')!='protocol'
    ORDER BY latest.timestamp DESC,latest.id DESC,latest.sender DESC LIMIT 1
)
WHERE c.account=? AND c.jid!='status@broadcast' AND (m.rowid IS NOT NULL OR NOT EXISTS(
    SELECT 1 FROM brazap_messages any_message WHERE any_message.account=c.account AND any_message.chat=c.jid
))
ORDER BY pinned DESC,updated_at DESC,c.jid
