-- Run after capacity_probe.sql and the probe binary has initialized the schema/indexes.
-- These plans should stay index-driven as the synthetic fleet grows.
EXPLAIN (ANALYZE, BUFFERS)
SELECT chat_id
FROM settings
WHERE key = 'report_at' AND value = ANY (ARRAY['120', '119', '118']::text[])
  AND value <> '';

EXPLAIN (ANALYZE, BUFFERS)
SELECT night.chat_id
FROM settings AS night
WHERE night.key = 'night' AND night.value <> ''
  AND night.value ~ '^[0-9]{1,4}\|[0-9]{1,4}$'
  AND split_part(night.value, '|', 1) = ANY (ARRAY['120', '119', '118']::text[])
UNION
SELECT night.chat_id
FROM settings AS night
WHERE night.key = 'night' AND night.value <> ''
  AND night.value ~ '^[0-9]{1,4}\|[0-9]{1,4}$'
  AND split_part(night.value, '|', 2) = ANY (ARRAY['120', '119', '118']::text[]);

EXPLAIN (ANALYZE, BUFFERS)
SELECT chat_id, key
FROM settings
WHERE key LIKE 'imgf:%'
ORDER BY chat_id, key
LIMIT 256;

EXPLAIN (ANALYZE, BUFFERS)
SELECT key, count(*)
FROM settings
WHERE key = ANY (ARRAY['captcha', 'flood', 'voice']::text[])
GROUP BY key;

EXPLAIN (ANALYZE, BUFFERS)
SELECT chat_id, key
FROM settings
WHERE key LIKE 'badge:%'
ORDER BY chat_id, key
LIMIT 256;
