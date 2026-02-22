SELECT
    s.session_id,
    s.project_path,
    s.first_seen_at,
    s.last_seen_at,
    COUNT(m.uuid) AS message_count,
    (SELECT model FROM messages
     WHERE session_id = s.session_id AND model IS NOT NULL
     LIMIT 1) AS model
FROM sessions s
LEFT JOIN messages m ON m.session_id = s.session_id
GROUP BY s.session_id
ORDER BY s.last_seen_at DESC
LIMIT :limit
