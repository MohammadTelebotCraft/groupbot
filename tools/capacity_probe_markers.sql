-- Run only against the disposable capacity probe database after capacity_probe.sql.
INSERT INTO settings (chat_id, key, value)
SELECT -(1000000000000 + group_no), 'night_state', 'off'
FROM generate_series(1::BIGINT, 500000::BIGINT) AS groups(group_no)
ON CONFLICT (chat_id, key) DO NOTHING;

ANALYZE settings;
