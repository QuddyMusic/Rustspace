#!/bin/sh
# Стенд федерации: две пары инстансов со свежими базами.
#   пара «ориг»: оригинал X (3111) ↔ оригинал Y (3112)
#   пара «rust»: сервер на Rust X (3113) ↔ оригинал Y (3114)
# Один и тот же сценарий идёт по обеим парам, журналы сравниваются (fed.py).
# BS_FED_RUST_Y=1 — в паре «rust» и Y работает на Rust (проверка Rust ↔ Rust).
# BS_FED_PG=postgres://user@host:port — инстансы на Rust хранят данные в PostgreSQL
# (база bsfed<порт> создаётся заново).
ORIG=${ORIG:-/home/claude/work/backspace-orig}
REPO=${REPO:-$(cd "$(dirname "$0")/../.." && pwd)}
pkill -f 'tsx/esm src/index.ts' 2>/dev/null; pkill -f backspace-server 2>/dev/null; sleep 1
SECRET=kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk
orig() {
  rm -rf /tmp/bsfed$1; mkdir -p /tmp/bsfed$1/uploads
  (cd "$ORIG/packages/server" && PORT=$1 HOST=127.0.0.1 PUBLIC_ORIGIN=http://localhost:$1 JWT_SECRET=$SECRET DB_PATH=/tmp/bsfed$1/bs.db UPLOAD_DIR=/tmp/bsfed$1/uploads REGISTRATION_OPEN=true DIRECTORY_ENDPOINT= BACKSPACE_UPDATE_CHECK=false BACKUP_DISABLED=true DISABLE_RATE_LIMITS=true FEDERATION_ALLOW_PRIVATE_PEERS=true NODE_ENV=development setsid nohup node --import tsx/esm src/index.ts > /tmp/bsfed$1/log 2>&1 < /dev/null &)
}
rust() {
  rm -rf /tmp/bsfed$1; mkdir -p /tmp/bsfed$1/uploads
  DBURL=
  if [ -n "$BS_FED_PG" ]; then
    psql "$BS_FED_PG/postgres" -qc "DROP DATABASE IF EXISTS bsfed$1" -c "CREATE DATABASE bsfed$1" || exit 1
    DBURL="$BS_FED_PG/bsfed$1"
  fi
  (cd "$REPO" && DATABASE_URL=$DBURL PORT=$1 HOST=127.0.0.1 PUBLIC_ORIGIN=http://localhost:$1 JWT_SECRET=$SECRET DB_PATH=/tmp/bsfed$1/bs.db UPLOAD_DIR=/tmp/bsfed$1/uploads REGISTRATION_OPEN=true DIRECTORY_ENDPOINT= BACKSPACE_UPDATE_CHECK=false DISABLE_RATE_LIMITS=true FEDERATION_ALLOW_PRIVATE_PEERS=true BS_PASSWORD_HASH_COST=4 setsid nohup target/debug/backspace-server > /tmp/bsfed$1/log 2>&1 < /dev/null &)
}
orig 3111; orig 3112; rust 3113
if [ -n "$BS_FED_RUST_Y" ]; then rust 3114; else orig 3114; fi
for port in 3111 3112 3113 3114; do
  ok=
  for i in $(seq 1 90); do curl -sf 127.0.0.1:$port/api/instance/info >/dev/null && ok=1 && break; sleep 1; done
  [ -n "$ok" ] || { echo "инстанс на $port не поднялся"; tail -5 /tmp/bsfed$port/log; exit 1; }
done
echo up
