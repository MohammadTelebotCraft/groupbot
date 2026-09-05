-- Run only against a disposable PostgreSQL database.
-- This creates 500,000 synthetic groups with six representative settings rows each.
CREATE TABLE IF NOT EXISTS settings (
    chat_id BIGINT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (chat_id, key)
);

INSERT INTO settings (chat_id, key, value)
SELECT -(1000000000000 + group_no), setting.key, setting.value
FROM generate_series(1::BIGINT, 500000::BIGINT) AS groups(group_no)
CROSS JOIN (VALUES
    ('owner', '1'),
    ('flood', ''),
    ('flood_limit', '8'),
    ('night', '120|300'),
    ('imgf:synthetic', ''),
    ('voice', '')
) AS setting(key, value)
ON CONFLICT (chat_id, key) DO NOTHING;

ANALYZE settings;
