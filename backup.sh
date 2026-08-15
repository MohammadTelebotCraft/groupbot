#!/usr/bin/env bash
set -euo pipefail

shard="${1:-$(dirname "$0")}"
cd "$shard"
name=$(basename "$(pwd)")
set -a
. ./.env
set +a

DEST="${BACKUP_DIR:-$HOME/backups}"
KEEP_DAYS="${BACKUP_KEEP_DAYS:-14}"
mkdir -p "$DEST"

stamp=$(date +%Y%m%d-%H%M)
file="$DEST/groupbot-$name-$stamp.sql.gz"

pg_dump --no-owner "$DATABASE_URL" | gzip -9 > "$file.part"
mv "$file.part" "$file"

tar -czf "$DEST/sessions-$name-$stamp.tar.gz" -C "$(pwd)" \
    --ignore-failed-read groupbot.session cleaner.session 2>/dev/null || true

find "$DEST" -name 'groupbot-*.sql.gz' -mtime "+$KEEP_DAYS" -delete
find "$DEST" -name 'sessions-*.tar.gz' -mtime "+$KEEP_DAYS" -delete

echo "$(date -Is) backed up $(du -h "$file" | cut -f1) to $file"
