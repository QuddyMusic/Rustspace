#!/bin/sh
# Обновление rustspace-server по заявке администратора из клиента.
#
# Ставится packaging/server/setup.sh и работает от root, но сам сервер
# root-прав не получает: он только кладёт заявку request.json в каталог
# помощника. Отсюда:
#   - выпуск берётся только из репозитория, заданного при установке
#     (GITHUB_REPO в updater.conf), а не из заявки;
#   - архив сверяется с опубликованной SHA-256;
#   - перед заменой — резервная копия базы и прежнего файла сервера;
#   - после запуска сервер должен ответить новой версией за минуту, иначе
#     возвращаются прежний файл и база.
# Ход виден администратору: status.json читает сервер.
#
# Запуск: из systemd (rustspace-update.path) или OpenRC (`--watch`).
set -eu

CONF=/etc/rustspace/updater.conf
[ -r "$CONF" ] || { echo "нет $CONF" >&2; exit 1; }
# shellcheck disable=SC1090
. "$CONF"
: "${APP:?}" "${BIN:?}" "${ENV_FILE:?}" "${UPDATER_DIR:?}" "${GITHUB_REPO:?}" "${RUST_ARCH:?}" "${INIT:?}" "${PORT:?}" "${BACKUPS:?}"

# Зеркало выпусков (по умолчанию GitHub).
RELEASE_BASE=${RELEASE_BASE:-https://github.com}
API_BASE=${API_BASE:-https://api.github.com}
REQUEST="$UPDATER_DIR/request.json"
STATUS="$UPDATER_DIR/status.json"
LOCK="$UPDATER_DIR/.lock"

now_ms() { echo "$(date +%s)000"; }

# Состояние для сервера: шаг — код (клиент переводит сам), версии — только
# проверенные строки, поэтому JSON пишется без экранирования.
# В POSIX sh переменные общие: у функций — свои имена с префиксом.
status() {
    st_file="$STATUS.tmp"
    printf '{"state":"%s","target":"%s","from":"%s","step":"%s","backup":"%s","at":%s}\n' \
        "$1" "$2" "${FROM:-}" "$3" "${4:-}" "$(now_ms)" > "$st_file"
    chmod 644 "$st_file"
    mv -f "$st_file" "$STATUS"
}

service() {
    if [ "$INIT" = systemd ]; then systemctl "$1" "$APP"; else rc-service "$APP" "$1"; fi
}

current_version() {
    curl -fsS --max-time 3 "http://127.0.0.1:$PORT/api/instance" 2>/dev/null \
        | sed -n 's/.*"version" *: *"\([^"]*\)".*/\1/p'
}

valid_version() {
    printf '%s' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+.][0-9A-Za-z.]+)?$'
}

# Резервная копия базы в $1 (сервер уже остановлен).
backup_db() {
    bk_dir=$1
    set -a
    # shellcheck disable=SC1090
    . "$ENV_FILE"
    set +a
    if [ -n "${DATABASE_URL:-}" ]; then
        command -v pg_dump >/dev/null 2>&1 || return 1
        case "${DB_CHOICE:-}" in
            local) (cd / && su postgres -s /bin/sh -c "pg_dump -Fc $APP") > "$bk_dir/db.dump" ;;
            *) pg_dump -Fc "$DATABASE_URL" > "$bk_dir/db.dump" ;;
        esac
    else
        [ -n "${DB_PATH:-}" ] && [ -f "$DB_PATH" ] || return 1
        for suffix in "" -wal -shm; do
            [ -f "$DB_PATH$suffix" ] && cp -p "$DB_PATH$suffix" "$bk_dir/db.sqlite$suffix"
        done
    fi
    chmod -R go-rwx "$bk_dir"
}

restore_db() {
    rs_dir=$1
    set -a
    # shellcheck disable=SC1090
    . "$ENV_FILE"
    set +a
    if [ -f "$rs_dir/db.dump" ]; then
        case "${DB_CHOICE:-}" in
            local) (cd / && su postgres -s /bin/sh -c "pg_restore --clean --if-exists -d $APP") < "$rs_dir/db.dump" ;;
            *) pg_restore --clean --if-exists -d "$DATABASE_URL" "$rs_dir/db.dump" ;;
        esac
    elif [ -f "$rs_dir/db.sqlite" ]; then
        rm -f "$DB_PATH-wal" "$DB_PATH-shm"
        for suffix in "" -wal -shm; do
            [ -f "$rs_dir/db.sqlite$suffix" ] && cp -p "$rs_dir/db.sqlite$suffix" "$DB_PATH$suffix"
        done
    fi
}

wait_version() {
    wv_i=0
    while [ $wv_i -lt 60 ]; do
        [ "$(current_version)" = "$1" ] && return 0
        wv_i=$((wv_i + 1))
        sleep 1
    done
    return 1
}

run_update() {
    raw=$(cat "$REQUEST" 2>/dev/null || true)
    rm -f "$REQUEST"
    wanted=$(printf '%s' "$raw" | sed -n 's/.*"version" *: *"\([^"]*\)".*/\1/p')
    [ -n "$wanted" ] || wanted=latest
    FROM=$(current_version || true)
    if [ "$wanted" = latest ]; then
        status running latest looking
        wanted=$(curl -fsSL --max-time 20 "$API_BASE/repos/$GITHUB_REPO/releases/latest" \
            | sed -n 's/.*"tag_name" *: *"v\{0,1\}\([^"]*\)".*/\1/p' | sed -n 1p)
    fi
    if ! valid_version "$wanted"; then
        status failed "" bad-version
        return 0
    fi
    if [ "$wanted" = "$FROM" ]; then
        status "done" "$wanted" same-version
        return 0
    fi
    asset="rustspace-server-linux-$RUST_ARCH.tar.gz"
    url="$RELEASE_BASE/$GITHUB_REPO/releases/download/v$wanted/$asset"
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    status running "$wanted" downloading
    if ! curl -fsSL --max-time 600 -o "$tmp/$asset" "$url" \
        || ! curl -fsSL --max-time 60 -o "$tmp/$asset.sha256" "$url.sha256"; then
        status failed "$wanted" download-failed
        return 0
    fi
    expected=$(cut -d' ' -f1 < "$tmp/$asset.sha256")
    actual=$(sha256sum "$tmp/$asset" | cut -d' ' -f1)
    if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
        status failed "$wanted" checksum
        return 0
    fi
    # Архив распаковывается целиком: имя файла внутри бывает и
    # «rustspace-server», и «./rustspace-server» (выпуск 0.1.1).
    mkdir "$tmp/x"
    found=""
    if tar xzf "$tmp/$asset" -C "$tmp/x"; then
        for candidate in "$tmp/x/rustspace-server" "$tmp/x"/*/rustspace-server; do
            if [ -f "$candidate" ] && [ -s "$candidate" ]; then
                found=$candidate
                break
            fi
        done
    fi
    if [ -z "$found" ]; then
        status failed "$wanted" no-binary
        return 0
    fi
    mv "$found" "$tmp/rustspace-server"
    stamp=$(date +%Y%m%d-%H%M%S)
    backup="$BACKUPS/$APP-$stamp-update"
    mkdir -m 700 "$backup"
    status running "$wanted" backup
    service stop || true
    cp -p "$BIN" "$backup/rustspace-server"
    if ! backup_db "$backup"; then
        service start || true
        status failed "$wanted" backup-failed
        return 0
    fi
    install -m 755 -o root -g root "$tmp/rustspace-server" "$BIN"
    status running "$wanted" starting
    service start || true
    if wait_version "$wanted"; then
        # Помощник обновляет и себя — из того же проверенного архива.
        if [ -s "$tmp/x/rustspace-update.sh" ] && sh -n "$tmp/x/rustspace-update.sh"; then
            install -m 755 -o root -g root "$tmp/x/rustspace-update.sh" "$(readlink -f "$0")" || true
        fi
        status "done" "$wanted" updated "$backup"
    else
        service stop || true
        install -m 755 -o root -g root "$backup/rustspace-server" "$BIN"
        restore_db "$backup" || true
        service start || true
        status rolled-back "$wanted" rolled-back "$backup"
    fi
    # Пять последних копий (имена — наши, с датой).
    # shellcheck disable=SC2012
    ls -1t "$BACKUPS" 2>/dev/null | tail -n +6 | while read -r old; do rm -rf "${BACKUPS:?}/$old"; done
}

once() {
    [ -f "$REQUEST" ] || return 0
    mkdir "$LOCK" 2>/dev/null || return 0
    trap 'rmdir "$LOCK" 2>/dev/null || true' EXIT
    run_update || status failed "" helper-error
    rmdir "$LOCK" 2>/dev/null || true
}

if [ "${1:-}" = "--watch" ]; then
    # OpenRC: путей-триггеров нет, смотрим раз в 10 секунд.
    while :; do once; sleep 10; done
else
    once
fi
