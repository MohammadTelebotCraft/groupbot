WITH batch AS (
                 SELECT chat, member, who, added
                 FROM UNNEST($1::bigint[], $2::bigint[], $3::text[], $4::bigint[])
                      AS incoming(chat, member, who, added)
             ), existing AS (
                 SELECT chat_id, count(*)::bigint AS members
                 FROM counters
                 WHERE chat_id = ANY($1::bigint[])
                 GROUP BY chat_id
             ), existing_batch AS (
                 SELECT batch.chat, batch.member, batch.who, batch.added
                 FROM batch
                 WHERE EXISTS (
                     SELECT 1 FROM counters
                     WHERE counters.chat_id = batch.chat
                       AND counters.user_id = batch.member
                 )
             ), new_batch AS (
                 SELECT batch.chat, batch.member, batch.who, batch.added,
                        row_number() OVER (
                            PARTITION BY batch.chat ORDER BY batch.member
                        ) AS new_rank
                 FROM batch
                 WHERE NOT EXISTS (
                     SELECT 1 FROM counters
                     WHERE counters.chat_id = batch.chat
                       AND counters.user_id = batch.member
                 )
             ), allowed_new AS (
                 SELECT new_batch.chat, new_batch.member, new_batch.who, new_batch.added
                 FROM new_batch
                 LEFT JOIN existing ON existing.chat_id = new_batch.chat
                 WHERE coalesce(existing.members, 0) + new_batch.new_rank <= $8
             ), reserved AS (
                 UPDATE durable_counts
                 SET counter_rows = counter_rows + (SELECT count(*) FROM allowed_new)
                 WHERE id = 0
                   AND counter_rows + (SELECT count(*) FROM allowed_new) <= $9
                 RETURNING id
             ), write_batch AS (
                 SELECT chat, member, who, added FROM existing_batch
                 UNION ALL
                 SELECT chat, member, who, added
                 FROM allowed_new
                 WHERE EXISTS (SELECT 1 FROM reserved)
             )
             INSERT INTO counters
                 (chat_id, user_id, name, total, today, day, week, week_at, month, month_at, seen)
             SELECT chat, member, who, added, added, $5, added, $6, added, $7, $5
             FROM write_batch
             ON CONFLICT (chat_id, user_id) DO UPDATE SET
                 name  = COALESCE(NULLIF(EXCLUDED.name, ''), counters.name),
                 total = counters.total + EXCLUDED.total,
                 today = CASE WHEN counters.day = EXCLUDED.day
                              THEN counters.today ELSE 0 END + EXCLUDED.today,
                 day   = EXCLUDED.day,
                 week  = CASE WHEN counters.week_at = EXCLUDED.week_at
                              THEN counters.week ELSE 0 END + EXCLUDED.week,
                 week_at = EXCLUDED.week_at,
                 month = CASE WHEN counters.month_at = EXCLUDED.month_at
                              THEN counters.month ELSE 0 END + EXCLUDED.month,
                 month_at = EXCLUDED.month_at,
                 seen  = EXCLUDED.seen
             RETURNING chat_id, user_id, name, total, awarded
