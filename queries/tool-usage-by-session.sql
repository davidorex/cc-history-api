SELECT
    te.tool_name,
    COUNT(*) as invocations,
    SUM(CASE WHEN te.is_error = 1 THEN 1 ELSE 0 END) as errors
FROM tool_executions te
JOIN messages m ON m.uuid = te.message_uuid
WHERE m.session_id = :session_id
GROUP BY te.tool_name
ORDER BY invocations DESC
