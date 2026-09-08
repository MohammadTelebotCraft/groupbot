#!/bin/sh
set -eu
PG_BIN=/usr/lib/postgresql/16/bin
AUDIT_PG_DIR="$HOME/groupbot-audit-pg"
if [ ! -f "$AUDIT_PG_DIR/PG_VERSION" ]; then
    "$PG_BIN/initdb" -D "$AUDIT_PG_DIR" --auth=trust > /dev/null
fi
if ! "$PG_BIN/pg_ctl" -D "$AUDIT_PG_DIR" status > /dev/null 2>&1; then
    "$PG_BIN/pg_ctl" -D "$AUDIT_PG_DIR" -l "$AUDIT_PG_DIR/server.log" \
      -o "-p 55432 -h 127.0.0.1 -k $AUDIT_PG_DIR -c shared_preload_libraries=pg_stat_statements -c track_io_timing=on" start
fi
if ! psql -h 127.0.0.1 -p 55432 -d postgres -Atqc "SELECT 1 FROM pg_database WHERE datname = 'groupbot_audit'" | grep -q 1; then
    createdb -h 127.0.0.1 -p 55432 groupbot_audit
fi
psql -h 127.0.0.1 -p 55432 -d groupbot_audit -c 'CREATE EXTENSION IF NOT EXISTS pg_stat_statements'
