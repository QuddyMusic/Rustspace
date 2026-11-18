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
# Кроме обновления, помощник выполняет заявки job-request.json (job.json —
# ход): резервная копия сейчас, удаление копии, другой каталог для копий,
# перенос файлов пользователей на другой диск. Список копий с размерами —
# backups.json. Сервер root-прав не получает, поэтому каждая заявка
# проверяется здесь заново: имена и пути — только простые, каталоги
# создаются свои (rustspace-backups, rustspace-uploads), чужие не трогаются.
# Сетевые хранилища (NFS, SMB, WebDAV) монтирует администратор в системе,
# в клиенте он только выбирает точку монтирования.
#
# Запуск: из systemd (rustspace-update.path) или OpenRC (`--watch`);
# `--scan` — только обновить список копий.
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
JOB_REQUEST="$UPDATER_DIR/job-request.json"
JOB="$UPDATER_DIR/job.json"
BACKUP_LIST="$UPDATER_DIR/backups.json"
LOCK="$UPDATER_DIR/.lock"

now_ms() { echo "$(date +%s)000"; }

# Файл для сервера: каталог помощника принадлежит серверу, поэтому пишем в
# новый файл (mktemp не пойдёт по подложенной ссылке) и подменяем целиком.
put_json() {
    pj_tmp=$(mktemp "$UPDATER_DIR/.put.XXXXXX")
    printf '%s\n' "$2" > "$pj_tmp"
    chmod 644 "$pj_tmp"
    mv -f "$pj_tmp" "$1"
}

# Состояние для сервера: шаг — код (клиент переводит сам), версии — только
# проверенные строки, поэтому JSON пишется без экранирования.
# В POSIX sh переменные общие: у функций — свои имена с префиксом.
status() {
    put_json "$STATUS" "$(printf '{"state":"%s","target":"%s","from":"%s","step":"%s","backup":"%s","at":%s}' \
        "$1" "$2" "${FROM:-}" "$3" "${4:-}" "$(now_ms)")"
}

# Ход заявки: job state step.
job_status() {
    put_json "$JOB" "$(printf '{"job":"%s","state":"%s","step":"%s","at":%s}' "$1" "$2" "$3" "$(now_ms)")"
}

# Имя копии: буквы, цифры, «.», «_», «-», не с точки.
safe_name() {
    printf '%s' "$1" | grep -Eq '^[A-Za-z0-9_][A-Za-z0-9._-]{0,127}$'
}

# Каталог, куда можно положить свой подкаталог: существующий, без ссылок
# и «..», не системный.
safe_dir() {
    case "$1" in /*) ;; *) return 1 ;; esac
    printf '%s' "$1" | grep -Eq '^[A-Za-z0-9._/ -]{1,255}$' || return 1
    case "$1/" in */../* | */./* | //*) return 1 ;; esac
    case "$1" in
        / | /etc* | /usr* | /bin* | /sbin* | /lib* | /boot* | /dev* | /proc* | /sys* | /run* | /root*) return 1 ;;
    esac
    [ -d "$1" ] && [ ! -L "$1" ] && [ "$(cd "$1" && pwd -P)" = "$1" ]
}

# backups.json: каталог копий и каждая копия с размером и временем.
write_backups() {
    wb_items=""
    if [ -d "$BACKUPS" ]; then
        # Новые сверху; имена проверяются ниже, пробелов в своих нет.
        # shellcheck disable=SC2045
        for wb_name in $(ls -1t "$BACKUPS" 2>/dev/null); do
            safe_name "$wb_name" || continue
            wb_kb=$(du -sk "$BACKUPS/$wb_name" 2>/dev/null | cut -f1)
            wb_at=$(stat -c %Y "$BACKUPS/$wb_name" 2>/dev/null || echo 0)
            wb_items="$wb_items${wb_items:+,}$(printf '{"name":"%s","size":%s,"at":%s000}' \
                "$wb_name" "$((${wb_kb:-0} * 1024))" "$wb_at")"
        done
    fi
    put_json "$BACKUP_LIST" "$(printf '{"dir":"%s","at":%s,"items":[%s]}' "$BACKUPS" "$(now_ms)" "$wb_items")"
}

service() {
    if [ "$INIT" = systemd ]; then systemctl "$1" "$APP"; else rc-service "$APP" "$1"; fi
}

current_version() {
    curl -fsS --max-time 3 "http://127.0.0.1:$PORT/api/instance/info" 2>/dev/null \
        | sed -n 's/.*"version" *: *"\([^"]*\)".*/\1/p'
}

valid_version() {
    printf '%s' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+.][0-9A-Za-z.]+)?$'
}

load_env() {
    set -a
    # shellcheck disable=SC1090
    . "$ENV_FILE"
    set +a
}

# Резервная копия базы в $1 (сервер уже остановлен).
backup_db() {
    bk_dir=$1
    load_env
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
    load_env
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
            install -m 755 -o root -g root "$tmp/x/rustspace-update.sh" "$(readlink -f "$0")" \
                && "$(readlink -f "$0")" --scan || true
        fi
        status "done" "$wanted" updated "$backup"
    else
        service stop || true
        install -m 755 -o root -g root "$backup/rustspace-server" "$BIN"
        restore_db "$backup" || true
        service start || true
        status rolled-back "$wanted" rolled-back "$backup"
    fi
    # Пять последних копий (имена — наши, с датой); сделанные вручную
    # удаляет только администратор.
    # shellcheck disable=SC2010
    ls -1t "$BACKUPS" 2>/dev/null | grep -v -- '-manual$' | tail -n +6 \
        | while read -r old; do rm -rf "${BACKUPS:?}/$old"; done
    write_backups
}

uploads_dir() {
    load_env
    case "${UPLOAD_DIR:-}" in
        /*) echo "$UPLOAD_DIR" ;;
        "") echo "${UPDATER_DIR%/updater}/data/uploads" ;;
        *) echo "${UPDATER_DIR%/updater}/$UPLOAD_DIR" ;;
    esac
}

# Резервная копия по кнопке: база без остановки сервера (SQLite — через
# sqlite3 .backup, если он есть, иначе короткая остановка), с «files» —
# ещё и файлы пользователей.
job_backup() {
    jb_dir="$BACKUPS/$APP-$(date +%Y%m%d-%H%M%S)-manual"
    mkdir -p "$BACKUPS" && chmod 700 "$BACKUPS"
    mkdir -m 700 "$jb_dir" || { job_status backup failed backup-failed; return 0; }
    job_status backup running backup
    load_env
    jb_ok=0
    if [ -z "${DATABASE_URL:-}" ] && command -v sqlite3 >/dev/null 2>&1 && [ -f "${DB_PATH:-}" ]; then
        sqlite3 "$DB_PATH" ".backup '$jb_dir/db.sqlite'" && jb_ok=1
        chmod -R go-rwx "$jb_dir"
    elif [ -n "${DATABASE_URL:-}" ]; then
        backup_db "$jb_dir" && jb_ok=1
    else
        service stop || true
        backup_db "$jb_dir" && jb_ok=1
        service start || true
    fi
    if [ "$jb_ok" = 1 ] && [ "$1" = files ]; then
        job_status backup running files
        jb_up=$(uploads_dir)
        tar czf "$jb_dir/uploads.tar.gz" -C "$jb_up" . || jb_ok=0
        chmod 600 "$jb_dir/uploads.tar.gz" 2>/dev/null || true
    fi
    if [ "$jb_ok" = 1 ]; then
        job_status backup "done" backup-done
    else
        rm -rf "$jb_dir"
        job_status backup failed backup-failed
    fi
    write_backups
}

job_delete() {
    if ! safe_name "$1" || [ ! -e "$BACKUPS/$1" ]; then
        job_status delete failed not-found
        return 0
    fi
    rm -rf "${BACKUPS:?}/$1"
    job_status delete "done" deleted
    write_backups
}

# Копии — в <каталог>/rustspace-backups; прежние переезжают туда же.
job_backup_dir() {
    if ! safe_dir "$1"; then
        job_status backup-dir failed bad-path
        return 0
    fi
    bd_new="$1/rustspace-backups"
    if [ "$bd_new" = "$BACKUPS" ]; then
        job_status backup-dir "done" same-path
        return 0
    fi
    job_status backup-dir running moving
    if ! mkdir -p "$bd_new" || ! chown root:root "$bd_new" || ! chmod 700 "$bd_new"; then
        job_status backup-dir failed write-failed
        return 0
    fi
    probe="$bd_new/.probe"
    if ! : > "$probe"; then
        job_status backup-dir failed write-failed
        return 0
    fi
    rm -f "$probe"
    if [ -d "$BACKUPS" ]; then
        for bd_path in "$BACKUPS"/*; do
            bd_name=${bd_path##*/}
            safe_name "$bd_name" && [ -e "$bd_path" ] || continue
            mv "$BACKUPS/$bd_name" "$bd_new/" || { job_status backup-dir failed move-failed; write_backups; return 0; }
        done
    fi
    sed -i "s|^BACKUPS=.*|BACKUPS='$bd_new'|" "$CONF"
    BACKUPS=$bd_new
    job_status backup-dir "done" moved
    write_backups
}

# Файлы пользователей — в <каталог>/rustspace-uploads: сервер
# останавливается, файлы копируются и сверяются, настройка меняется; если
# сервер с новым каталогом не поднялся — всё возвращается. Прежние файлы
# удаляются только после успешного запуска.
job_upload_dir() {
    if ! safe_dir "$1"; then
        job_status upload-dir failed bad-path
        return 0
    fi
    ud_old=$(uploads_dir)
    ud_new="$1/rustspace-uploads"
    if [ "$ud_new" = "$ud_old" ]; then
        job_status upload-dir "done" same-path
        return 0
    fi
    if [ -e "$ud_new" ]; then
        job_status upload-dir failed target-exists
        return 0
    fi
    job_status upload-dir running stopping
    service stop || true
    mkdir -m 750 "$ud_new" || { service start || true; job_status upload-dir failed write-failed; return 0; }
    job_status upload-dir running copying
    if [ -d "$ud_old" ] && ! cp -Rp "$ud_old/." "$ud_new/"; then
        rm -rf "$ud_new"
        service start || true
        job_status upload-dir failed copy-failed
        return 0
    fi
    if [ -d "$ud_old" ] && [ "$(find "$ud_old" | wc -l)" != "$(find "$ud_new" | wc -l)" ]; then
        rm -rf "$ud_new"
        service start || true
        job_status upload-dir failed copy-failed
        return 0
    fi
    chown -R "$APP:$APP" "$ud_new"
    cp -p "$ENV_FILE" "$ENV_FILE.before-uploads"
    if grep -q '^UPLOAD_DIR=' "$ENV_FILE"; then
        sed -i "s|^UPLOAD_DIR=.*|UPLOAD_DIR='$ud_new'|" "$ENV_FILE"
    else
        printf "UPLOAD_DIR='%s'\n" "$ud_new" >> "$ENV_FILE"
    fi
    job_status upload-dir running starting
    service start || true
    if wait_running; then
        rm -f "$ENV_FILE.before-uploads"
        [ -d "$ud_old" ] && rm -rf "$ud_old"
        job_status upload-dir "done" moved
    else
        service stop || true
        mv -f "$ENV_FILE.before-uploads" "$ENV_FILE"
        rm -rf "$ud_new"
        service start || true
        job_status upload-dir failed start-failed
    fi
}

# Сервер отвечает (любой версией) за минуту.
wait_running() {
    wr_i=0
    while [ $wr_i -lt 60 ]; do
        [ -n "$(current_version)" ] && return 0
        wr_i=$((wr_i + 1))
        sleep 1
    done
    return 1
}

run_job() {
    raw=$(cat "$JOB_REQUEST" 2>/dev/null || true)
    rm -f "$JOB_REQUEST"
    job=$(printf '%s' "$raw" | sed -n 's/.*"job" *: *"\([a-z-]*\)".*/\1/p')
    arg=$(printf '%s' "$raw" | sed -n 's/.*"arg" *: *"\([^"]*\)".*/\1/p')
    case "$job" in
        backup) job_backup "$arg" ;;
        delete) job_delete "$arg" ;;
        backup-dir) job_backup_dir "$arg" ;;
        upload-dir) job_upload_dir "$arg" ;;
        scan) write_backups ;;
        *) job_status "" failed bad-job ;;
    esac
}

# Установки, сделанные до заявок, следят только за request.json: помощник,
# обновивший себя, добавляет в systemd и job-request.json.
ensure_watch() {
    [ "$INIT" = systemd ] || return 0
    ew_unit="/etc/systemd/system/$APP-update.path"
    [ -f "$ew_unit" ] || return 0
    grep -q 'job-request.json' "$ew_unit" && return 0
    sed -i "s|^PathExists=.*/request.json$|&\nPathExists=$UPDATER_DIR/job-request.json|" "$ew_unit"
    systemctl daemon-reload
    systemctl restart "$APP-update.path" || true
}

once() {
    [ -f "$REQUEST" ] || [ -f "$JOB_REQUEST" ] || return 0
    mkdir "$LOCK" 2>/dev/null || return 0
    trap 'rmdir "$LOCK" 2>/dev/null || true' EXIT
    if [ -f "$REQUEST" ]; then
        run_update || status failed "" helper-error
    fi
    if [ -f "$JOB_REQUEST" ]; then
        run_job || job_status "" failed helper-error
    fi
    rmdir "$LOCK" 2>/dev/null || true
}

case "${1:-}" in
    --watch)
        # OpenRC: путей-триггеров нет, смотрим раз в 10 секунд.
        while :; do once; sleep 10; done
        ;;
    --scan)
        ensure_watch
        write_backups
        ;;
    *) once ;;
esac
