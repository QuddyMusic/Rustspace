#!/bin/sh
# Пути стенда: checkout оригинала, этот репозиторий, каталог скриптов.
ORIG=${ORIG:-/home/claude/work/backspace-orig}
REPO=${REPO:-$(cd "$(dirname "$0")/../.." && pwd)}
STAND=${STAND:-$(cd "$(dirname "$0")" && pwd)}
# Оригинал на 3105 (свежая база), наполнение, затем Rust на 3106 с копией базы.
pkill -f 'tsx/esm src/index.ts' 2>/dev/null; pkill -f backspace-server 2>/dev/null; sleep 1
rm -rf /tmp/bsorig /tmp/bsrust; mkdir -p /tmp/bsorig/uploads /tmp/bsrust/uploads
start_orig() {
cd "$ORIG/packages/server"
PORT=3105 HOST=127.0.0.1 JWT_SECRET=kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk DB_PATH=/tmp/bsorig/bs.db UPLOAD_DIR=/tmp/bsorig/uploads REGISTRATION_OPEN=true DIRECTORY_ENDPOINT= BACKSPACE_UPDATE_CHECK=false BACKUP_DISABLED=true DISABLE_RATE_LIMITS=true LIVEKIT_URL=ws://127.0.0.1:7880 LIVEKIT_API_KEY=devkey LIVEKIT_API_SECRET=secret NODE_ENV=development setsid nohup node --import tsx/esm src/index.ts > /tmp/bsorig/log 2>&1 < /dev/null &
for i in $(seq 1 60); do curl -sf 127.0.0.1:3105/api/instance/info >/dev/null && return 0; sleep 1; done; echo "оригинал не поднялся"; exit 1
}
start_orig
python3 "$STAND/seed.py" || exit 1
pkill -f 'tsx/esm src/index.ts'; sleep 2
python3 - <<'P'
import sqlite3
c=sqlite3.connect('/tmp/bsorig/bs.db'); c.execute('pragma wal_checkpoint(truncate)'); c.close()
P
cp /tmp/bsorig/bs.db /tmp/bsrust/bs.db
start_orig
cd "$REPO"
PORT=3106 HOST=127.0.0.1 JWT_SECRET=kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk DB_PATH=/tmp/bsrust/bs.db UPLOAD_DIR=/tmp/bsrust/uploads REGISTRATION_OPEN=true DIRECTORY_ENDPOINT= BACKSPACE_UPDATE_CHECK=false DISABLE_RATE_LIMITS=true LIVEKIT_URL=ws://127.0.0.1:7880 LIVEKIT_API_KEY=devkey LIVEKIT_API_SECRET=secret BS_PASSWORD_HASH_COST=4 setsid nohup target/debug/backspace-server > /tmp/bsrust/log 2>&1 < /dev/null &
for i in $(seq 1 30); do curl -sf 127.0.0.1:3106/api/instance/info >/dev/null && echo up && exit 0; sleep 1; done; echo "rust не поднялся"; tail /tmp/bsrust/log; exit 1
