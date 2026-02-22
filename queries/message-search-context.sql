SELECT
    m.uuid,
    m.session_id,
    m.type AS message_type,
    m.timestamp,
    mc.block_type,
    SUBSTR(mc.text_content, 1, 200) as content_preview
FROM messages m
JOIN message_content mc ON m.uuid = mc.message_uuid
WHERE mc.text_content LIKE '%' || :search_term || '%'
ORDER BY m.timestamp DESC
LIMIT :limit
