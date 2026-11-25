#!/bin/sh
# Установка, обновление и перенос сервера Rustspace (bs-server, совместим с
# Backspace): Debian, Ubuntu или Alpine; systemd или OpenRC; база PostgreSQL;
# голос и стримы через LiveKit.
#
#   Установка:   sudo sh packaging/server/setup.sh --domain rustspace.duckdns.org
#   Обновление:  git pull && sudo sh packaging/server/setup.sh
#   Без клона:   curl -fsSL https://raw.githubusercontent.com/QuddyMusic/Rustspace/main/packaging/server/setup.sh \
#                  | sudo sh -s -- --domain rustspace.duckdns.org
#
# Что делает (повторный запуск безопасен: секреты и данные сохраняются):
#   1. ставит пакеты и PostgreSQL, заводит в нём базу и пользователя rustspace;
#   2. собирает rustspace-server из этого репозитория или скачивает готовый
#      из релиза (--release), проверяя контрольную сумму;
#   3. скачивает LiveKit (сервер голоса, видео и стримов) и проверяет его хэш;
#   4. пишет настройки в /etc/rustspace/ и заводит службы rustspace и livekit;
#   5. перед обновлением делает резервную копию базы в /var/backups/rustspace;
#   6. переносит данные: из базы SQLite (прежняя установка, запущенный вручную
#      сервер, --data-from) в PostgreSQL, а установку со старыми именами
#      (/etc/backspace, /var/lib/backspace, служба и пользователь backspace) —
#      на новые;
#   7. проверяет, что всё отвечает, и печатает, что настроить в NPM и на роутере.
#
# Схема: снаружи HTTPS делает обратный прокси (Nginx Proxy Manager, Caddy…):
#   https://DOMAIN     -> http://ЭТА_МАШИНА:3000  (Websockets Support вкл.)
#   https://lk.DOMAIN  -> http://ЭТА_МАШИНА:7880  (Websockets Support вкл.)
# Медиа LiveKit идёт мимо прокси: на роутере TCP 7881 и UDP 7882 на эту машину.

set -eu

# ─── Параметры ──────────────────────────────────────────────────────────────

DOMAIN=""
LIVEKIT_HOST=""
WITH_LIVEKIT=""
BINARY=""
RELEASE=""
PORT=""
PROXY_HOPS=""
WEB_DIST=""
WITH_WEB=""
SOURCE_URL=""
DATA_FROM=""
DB_CHOICE=""
EXTERNAL_DB_URL=""
TELEGRAM_APP=""
ASSUME_YES=0
JOBS=""
LIVEKIT_VERSION="1.13.11"

GITHUB_REPO="QuddyMusic/Rustspace"


# Хэши архивов LiveKit 1.13.11 (из checksums.txt релиза, сверены вручную):
# так подменённый архив не пройдёт, даже если подменён и checksums.txt.
pinned_livekit_sha() {
    case "$1" in
        amd64) echo 1e9ee90ca301e1842b2887fc77cb122759756a9c8aeb4334ee91de3325ec43fb ;;
        arm64) echo 2af6ce091d6b3c79d1677f8ff1bbb91a8007f40b2325c059b52d38383c0830b6 ;;
        armv7) echo 5f663a6498c3d81ef2dce55a3a1e7815aacfe78207e469abc0bd9c5b5ca5070b ;;
    esac
}

APP=rustspace
WITH_UPDATER=${WITH_UPDATER:-1}
ETC=/etc/rustspace
ENV_FILE=$ETC/rustspace.env
LK_FILE=$ETC/livekit.yaml
DATA=/var/lib/rustspace
BIN=/usr/local/bin/rustspace-server
LK_BIN=/usr/local/bin/livekit-server
BUILD=/opt/rustspace-build
LOG_DIR=/var/log/rustspace
BACKUPS=/var/backups/rustspace
# Каталог копий, выбранный администратором в клиенте, сохраняется.
if [ -r "$ETC/updater.conf" ]; then
    prev_backups=$(sed -n "s/^BACKUPS='\(.*\)'$/\1/p" "$ETC/updater.conf")
    [ -n "$prev_backups" ] && [ -d "$prev_backups" ] && BACKUPS=$prev_backups
fi
DB_FILE=$DATA/data/rustspace.db

# Установка до переименования в Rustspace.
OLD_NAME=backspace
OLD_ETC=/etc/backspace
OLD_ENV_FILE=$OLD_ETC/backspace.env
OLD_DATA=/var/lib/backspace
OLD_BIN=/usr/local/bin/backspace-server
OLD_BUILD=/opt/backspace-build
OLD_LOG_DIR=/var/log/backspace

# Переменные, которые сервер читает; переносятся из прежних настроек.
KNOWN_VARS="BACKSPACE_COMMIT BACKSPACE_INSTALL_CHANNEL BACKSPACE_SOURCE_URL
BACKSPACE_UPDATE_CHECK BS_EXTRA_CA_CERTS BS_PASSWORD_HASH_COST BS_WEB_DIST
DATABASE_URL DB_PATH DIRECTORY_ENDPOINT DISABLE_FEDERATION_WORKERS DOMAIN
FEDERATION_ALLOW_PRIVATE_PEERS HOST JWT_EXPIRES_IN JWT_SECRET LIVEKIT_API_KEY
LIVEKIT_API_SECRET LIVEKIT_URL MAX_UPLOAD_SIZE PORT PUBLIC_ORIGIN
REGISTRATION_OPEN TRUSTED_PROXY_HOPS TUS_EXPIRATION_HOURS UPLOAD_DIR
TELEGRAM_BOT_TOKEN TELEGRAM_API_URL DISCORD_BOT_TOKEN DISCORD_API_URL
MATRIX_HOMESERVER MATRIX_ACCESS_TOKEN PERSONAL_BRIDGES TELEGRAM_API_ID TELEGRAM_API_HASH"
# Их скрипт задаёт сам.
MANAGED_VARS="BS_UPDATER_DIR DOMAIN HOST PORT TRUSTED_PROXY_HOPS DB_PATH DATABASE_URL UPLOAD_DIR
JWT_SECRET LIVEKIT_URL LIVEKIT_API_KEY LIVEKIT_API_SECRET BS_WEB_DIST"

usage() {
    cat <<'EOF'
Использование: sudo sh setup.sh [--domain ДОМЕН] [параметры]

  --domain ДОМЕН         домен инстанса, например rustspace.duckdns.org
                         (при обновлении берётся из прежних настроек)
  --livekit-host ИМЯ     адрес LiveKit для клиентов (по умолчанию lk.ДОМЕН)
  --no-livekit           без голоса и стримов
  --livekit              включить голос и стримы (при обновлении установки,
                         где их не было; иначе они включены и так)

Откуда взять сервер (по умолчанию — сборка, если скрипт лежит в репозитории,
иначе последний релиз):
  --release [ВЕРСИЯ]     скачать готовый из релиза GitHub (latest или 0.3.0)
  --binary ПУТЬ          взять готовый файл rustspace-server
  --jobs N               потоков сборки (меньше — меньше памяти)

База данных (по умолчанию — PostgreSQL на этой машине):
  --sqlite               хранить данные в SQLite (/var/lib/rustspace/data/rustspace.db);
                         выбор запоминается и при обновлениях
  --postgres             перейти с SQLite на PostgreSQL (данные переносятся)
  --database-url URL     внешний PostgreSQL: postgres://user:pass@host/db

Прочее:
  --port ПОРТ            порт сервера (3000)
  --proxy-hops N         сколько прокси перед сервером (1; без прокси — 0)
  --web                  веб-версия Rustspace на https://ДОМЕН/: тот же клиент в
                         браузере, ею же открывается десктоп Backspace. Берётся
                         из того же релиза, что сервер (при сборке из исходников
                         — собирается), и обновляется вместе с ним. Можно и
                         позже: повторный запуск с --web
  --no-web               убрать веб-версию (на / снова страница «подключайтесь
                         клиентом»)
  --web-dist КАТАЛОГ     взять уже собранный веб-клиент (packaging/web/build.sh)
  --source-url URL       репозиторий с исходниками запущенного сервера, если вы
                         его меняли (по умолчанию — github.com/QuddyMusic/Rustspace)
  --data-from КАТАЛОГ    перенести данные из каталога (*.db и uploads/)
  --livekit-version В    версия LiveKit (1.13.11)
  --no-updater           не ставить помощника обновления из клиента
  --telegram-app ID:HASH ключи приложения Telegram (my.telegram.org) для личных
                         мостов — одни на весь сервер, пользователям свои не нужны
  -y, --yes              не задавать вопросов
  -h, --help             эта справка
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --domain) DOMAIN="${2:-}"; shift 2 ;;
        --livekit-host) LIVEKIT_HOST="${2:-}"; shift 2 ;;
        --no-livekit) WITH_LIVEKIT=0; shift ;;
        --livekit) WITH_LIVEKIT=1; shift ;;
        --binary) BINARY="${2:-}"; shift 2 ;;
        --release)
            case "${2:-}" in
                ''|-*) RELEASE=latest; shift ;;
                *) RELEASE="${2#v}"; shift 2 ;;
            esac ;;
        --port) PORT="${2:-}"; shift 2 ;;
        --proxy-hops) PROXY_HOPS="${2:-}"; shift 2 ;;
        --web-dist) WEB_DIST="${2:-}"; shift 2 ;;
        --web) WITH_WEB=1; shift ;;
        --no-web) WITH_WEB=0; shift ;;
        --source-url) SOURCE_URL="${2:-}"; shift 2 ;;
        --data-from) DATA_FROM="${2:-}"; shift 2 ;;
        --sqlite) DB_CHOICE=sqlite; shift ;;
        --postgres) DB_CHOICE=local; shift ;;
        --database-url) DB_CHOICE=external; EXTERNAL_DB_URL="${2:-}"; shift 2 ;;
        --livekit-version) LIVEKIT_VERSION="${2:-}"; shift 2 ;;
        --jobs) JOBS="${2:-}"; shift 2 ;;
        --no-updater) WITH_UPDATER=0; shift ;;
        --telegram-app) TELEGRAM_APP="${2:-}"; shift 2 ;;
        -y|--yes) ASSUME_YES=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Неизвестный параметр: $1" >&2; usage >&2; exit 2 ;;
    esac
done

# ─── Вспомогательное ────────────────────────────────────────────────────────

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31mОшибка:\033[0m %s\n' "$*" >&2; exit 1; }

# Вопрос читается с терминала: при `curl … | sh` stdin — это сам скрипт.
confirm() {
    [ "$ASSUME_YES" = 1 ] && return 0
    { : < /dev/tty; } 2>/dev/null || die "нет терминала для вопроса «$1» — запустите с -y"
    printf '%s [y/N] ' "$1" > /dev/tty
    read -r answer < /dev/tty || answer=""
    case "$answer" in y|Y|yes|д|Д|да) return 0 ;; *) return 1 ;; esac
}

# Адрес файла релиза: того, из которого ставится сервер.
release_url() {
    if [ "$RELEASE" = latest ]; then
        echo "https://github.com/$GITHUB_REPO/releases/latest/download/$1"
    else
        echo "https://github.com/$GITHUB_REPO/releases/download/v$RELEASE/$1"
    fi
}

# Скачивает файл релиза в каталог $2 и сверяет его контрольную сумму.
fetch_release() {
    url=$(release_url "$1")
    curl -sSfL -o "$2/$1" "$url" || die "не удалось скачать $url"
    curl -sSfL -o "$2/$1.sha256" "$url.sha256" || die "не удалось скачать $url.sha256"
    expected=$(cut -d' ' -f1 < "$2/$1.sha256")
    actual=$(sha256sum "$2/$1" | cut -d' ' -f1)
    [ -n "$expected" ] && [ "$actual" = "$expected" ] || die "контрольная сумма $1 не совпала"
}

# Ставит веб-версию Rustspace в $DATA/web: из релиза или, если сервер
# собирается из исходников, сборкой (packaging/web/build.sh).
install_web() {
    work=$(mktemp -d)
    if [ "$BUILDING" = 1 ]; then
        say "Собираю веб-версию Rustspace (первый раз — 5–15 минут)"
        (cd "$REPO" && PATH="$CARGO_HOME/bin:$PATH" sh packaging/web/build.sh "$work/web") \
            || die "сборка веб-версии не удалась"
    elif [ -n "$RELEASE" ]; then
        say "Скачиваю веб-версию Rustspace"
        fetch_release rustspace-web.tar.gz "$work"
        mkdir "$work/web"
        tar xzf "$work/rustspace-web.tar.gz" -C "$work/web"
    else
        die "веб-версия ставится из релиза или сборкой из исходников; с --binary укажите --web-dist"
    fi
    [ -f "$work/web/index.html" ] && [ -f "$work/web/rustspace_bg.wasm" ] \
        || die "в веб-версии нет index.html или rustspace_bg.wasm"
    rm -rf "$DATA/web"
    cp -R "$work/web" "$DATA/web"
    chown -R "root:$APP" "$DATA/web"
    chmod -R g+rX,o-rwx "$DATA/web"
    rm -rf "$work"
}

random_hex() { od -An -N"$1" -tx1 /dev/urandom | tr -d ' \n'; }

# Значение переменной из файла KEY='value' или KEY=value.
file_value() {
    [ -f "$2" ] || return 0
    sed -n "s/^$1='\(.*\)'\$/\1/p; s/^$1=\([^']*\)\$/\1/p" "$2" | tail -n 1
}

# Значение переменной окружения процесса.
proc_value() {
    tr '\0' '\n' < "/proc/$2/environ" 2>/dev/null | sed -n "s/^$1=//p" | tail -n 1
}

is_managed() {
    for m in $MANAGED_VARS; do [ "$m" = "$1" ] && return 0; done
    return 1
}

check_value() {
    case "$2" in
        *"'"*|*'
'*) die "значение $1 содержит кавычку или перевод строки — задайте его вручную в $ENV_FILE" ;;
    esac
}

# Команда от имени postgres (через сокет, без пароля). Каталог — /, чтобы
# psql не ругался на недоступный текущий каталог root.
as_postgres() { (cd / && su postgres -s /bin/sh -c "$1"); }
pg_sql() { as_postgres "psql -qAtX -v ON_ERROR_STOP=1 -c \"$1\""; }

# Запуск сервера с его настройками (для import-sqlite).
with_server_env() {
    (
        set -a
        # shellcheck source=/dev/null
        . "$ENV_FILE"
        set +a
        cd "$DATA"
        "$@"
    )
}

# ─── Проверки ───────────────────────────────────────────────────────────────

[ "$(id -u)" = 0 ] || die "запустите от root: sudo sh $0"
[ -r /etc/os-release ] || die "не удалось определить систему (/etc/os-release)"
# shellcheck source=/dev/null
. /etc/os-release
case " ${ID:-} ${ID_LIKE:-} " in
    *" alpine "*) OS=alpine ;;
    *" debian "*|*" ubuntu "*) OS=debian ;;
    *) die "поддерживаются Debian, Ubuntu и Alpine, а здесь ${PRETTY_NAME:-неизвестная система}" ;;
esac

if [ -d /run/systemd/system ]; then
    INIT=systemd
elif command -v openrc-run >/dev/null 2>&1; then
    INIT=openrc
else
    INIT=none
fi

case "$(uname -m)" in
    x86_64|amd64) ARCH=amd64; RUST_ARCH=x86_64 ;;
    aarch64|arm64) ARCH=arm64; RUST_ARCH=aarch64 ;;
    armv7l|armv7*) ARCH=armv7; RUST_ARCH="" ;;
    *) die "архитектура $(uname -m) не поддерживается" ;;
esac

SCRIPT_DIR=$(cd "$(dirname "$0")" 2>/dev/null && pwd || echo /)
REPO=$(cd "$SCRIPT_DIR/../.." 2>/dev/null && pwd || echo /)
IN_REPO=0
[ -f "$REPO/crates/bs-server/Cargo.toml" ] && IN_REPO=1
if [ -z "$BINARY" ] && [ -z "$RELEASE" ] && [ "$IN_REPO" = 0 ]; then
    RELEASE=latest
fi
if [ -n "$RELEASE" ] && [ -z "$RUST_ARCH" ]; then
    die "готовых сборок для $(uname -m) нет: склонируйте репозиторий и запустите скрипт из него"
fi
[ -z "$BINARY" ] || [ -x "$BINARY" ] || die "$BINARY — не исполняемый файл"
[ -z "$WEB_DIST" ] || [ -f "$WEB_DIST/index.html" ] || die "в $WEB_DIST нет index.html"
[ -z "$DATA_FROM" ] || [ -d "$DATA_FROM" ] || die "нет каталога $DATA_FROM"
if [ "$DB_CHOICE" = external ]; then
    case "$EXTERNAL_DB_URL" in
        postgres://*|postgresql://*) ;;
        *) die "--database-url должен начинаться с postgres://" ;;
    esac
fi

# ─── Прежние настройки ──────────────────────────────────────────────────────

CARRY=$(mktemp)
NEW_BIN=$(mktemp)
trap 'rm -f "$CARRY" "$NEW_BIN"' EXIT
OLD_PID=""
OLD_CWD=""
LEGACY=0
UPDATE=0

carry_file() {
    for v in $KNOWN_VARS; do
        value=$(file_value "$v" "$1")
        [ -n "$value" ] && printf '%s=%s\n' "$v" "$value" >> "$CARRY"
    done
    return 0
}

if [ -f "$ENV_FILE" ]; then
    UPDATE=1
    say "Найдена установка ($ENV_FILE): обновляю, секреты и данные сохраняются"
    carry_file "$ENV_FILE"
elif [ -f "$OLD_ENV_FILE" ]; then
    UPDATE=1
    LEGACY=1
    say "Найдена установка со старыми именами ($OLD_ENV_FILE): переношу на rustspace"
    carry_file "$OLD_ENV_FILE"
else
    # Сервер, запущенный вручную: его настройки и данные — основа новой установки.
    # Ищем по файлу программы: имя процесса ядро обрезает до 15 знаков.
    for dir in /proc/[0-9]*; do
        exe=$(readlink "$dir/exe" 2>/dev/null) || continue
        case "$exe" in
            "$BIN"|"$BIN (deleted)"|"$OLD_BIN"|"$OLD_BIN (deleted)") continue ;;
            */backspace-server|*"/backspace-server (deleted)") ;;
            */rustspace-server|*"/rustspace-server (deleted)") ;;
            *) continue ;;
        esac
        OLD_PID=${dir#/proc/}
        OLD_CWD=$(readlink "$dir/cwd" 2>/dev/null || echo /)
        break
    done
    if [ -n "$OLD_PID" ]; then
        say "Найден запущенный вручную сервер (pid $OLD_PID, каталог $OLD_CWD): беру его настройки"
        for v in $KNOWN_VARS; do
            value=$(proc_value "$v" "$OLD_PID")
            [ -n "$value" ] && printf '%s=%s\n' "$v" "$value" >> "$CARRY"
        done
    fi
fi

carried() { sed -n "s/^$1=//p" "$CARRY" | tail -n 1; }

# Что не задано параметрами — из прежних настроек или по умолчанию.
[ -n "$DOMAIN" ] || DOMAIN=$(carried DOMAIN)
[ -n "$DOMAIN" ] || { usage >&2; die "не указан --domain"; }
case "$DOMAIN" in
    http://*|https://*|*/*) die "--domain — только имя, без https:// и пути: ${DOMAIN#*://}" ;;
esac
[ -n "$PORT" ] || PORT=$(carried PORT)
[ -n "$PORT" ] || PORT=3000
case "$PORT" in *[!0-9]*) die "--port должен быть числом" ;; esac
[ -n "$PROXY_HOPS" ] || PROXY_HOPS=$(carried TRUSTED_PROXY_HOPS)
[ -n "$PROXY_HOPS" ] || PROXY_HOPS=1
case "$PROXY_HOPS" in [0-4]) ;; *) die "--proxy-hops — от 0 до 4" ;; esac
OLD_LK_URL=$(carried LIVEKIT_URL)
if [ -z "$WITH_LIVEKIT" ]; then
    if [ "$UPDATE" = 1 ] && [ -z "$OLD_LK_URL" ]; then WITH_LIVEKIT=0; else WITH_LIVEKIT=1; fi
fi
if [ -z "$LIVEKIT_HOST" ]; then
    case "$OLD_LK_URL" in
        wss://*) LIVEKIT_HOST=${OLD_LK_URL#wss://}; LIVEKIT_HOST=${LIVEKIT_HOST%%/*} ;;
        *) LIVEKIT_HOST="lk.$DOMAIN" ;;
    esac
fi

JWT_SECRET=$(carried JWT_SECRET)
if [ -z "$JWT_SECRET" ]; then
    if [ -n "$OLD_PID" ] || [ -n "$DATA_FROM" ] || [ "$UPDATE" = 1 ]; then
        warn "у прежнего сервера не найден JWT_SECRET: с новым всем придётся войти заново,"
        warn "а двухфакторная защита у пользователей перестанет работать"
        confirm "Продолжить с новым JWT_SECRET?" || die "остановлено; задайте JWT_SECRET в $ENV_FILE и запустите снова"
    fi
    JWT_SECRET=$(random_hex 32)
fi
check_value JWT_SECRET "$JWT_SECRET"

LK_KEY=$(carried LIVEKIT_API_KEY)
LK_SECRET=$(carried LIVEKIT_API_SECRET)
# Прежние ключи LiveKit годятся, только если это он же (тот же адрес).
if [ "$WITH_LIVEKIT" = 1 ]; then
    if [ -z "$LK_KEY" ] || [ -z "$LK_SECRET" ] || [ "$OLD_LK_URL" != "wss://$LIVEKIT_HOST" ]; then
        LK_KEY="API$(random_hex 6)"
        LK_SECRET=$(random_hex 32)
    fi
    check_value LIVEKIT_API_KEY "$LK_KEY"
    check_value LIVEKIT_API_SECRET "$LK_SECRET"
fi

# ─── База данных ────────────────────────────────────────────────────────────

# Адрес локальной базы, заведённой этим скриптом.
LOCAL_DB_PREFIX="postgres://$APP:"
LOCAL_DB_SUFFIX="@127.0.0.1:5432/$APP?sslmode=disable"

OLD_DB_URL=$(carried DATABASE_URL)
# SQLite, выбранный явно (--sqlite), остаётся при обновлениях; прочие установки
# на SQLite (старые, запущенные вручную) переходят на PostgreSQL.
if [ -z "$DB_CHOICE" ] && [ -f "$ENV_FILE" ] && grep -q '^# setup: --sqlite$' "$ENV_FILE"; then
    DB_CHOICE=sqlite
fi
if [ -z "$DB_CHOICE" ]; then
    case "$OLD_DB_URL" in
        "$LOCAL_DB_PREFIX"*"$LOCAL_DB_SUFFIX") DB_CHOICE=local ;;
        postgres://*|postgresql://*) DB_CHOICE=external; EXTERNAL_DB_URL=$OLD_DB_URL ;;
        *) DB_CHOICE=local ;;
    esac
fi
DB_PASSWORD=""
case "$OLD_DB_URL" in
    "$LOCAL_DB_PREFIX"*"$LOCAL_DB_SUFFIX")
        DB_PASSWORD=${OLD_DB_URL#"$LOCAL_DB_PREFIX"}
        DB_PASSWORD=${DB_PASSWORD%"$LOCAL_DB_SUFFIX"} ;;
esac
[ -n "$DB_PASSWORD" ] || DB_PASSWORD=$(random_hex 24)
case "$DB_CHOICE" in
    local) DATABASE_URL="$LOCAL_DB_PREFIX$DB_PASSWORD$LOCAL_DB_SUFFIX" ;;
    external) DATABASE_URL=$EXTERNAL_DB_URL ;;
    *) DATABASE_URL="" ;;
esac
[ -z "$DATABASE_URL" ] || check_value DATABASE_URL "$DATABASE_URL"

# Где лежат данные, которые надо перенести: база SQLite и файлы.
SRC_DB=""
SRC_UPLOADS=""
if [ -n "$DATA_FROM" ]; then
    for f in "$DATA_FROM/rustspace.db" "$DATA_FROM/backspace.db" "$DATA_FROM"/*.db; do
        [ -f "$f" ] && { SRC_DB=$f; break; }
    done
    SRC_UPLOADS="$DATA_FROM/uploads"
elif [ -n "$OLD_PID" ]; then
    db=$(carried DB_PATH)
    if [ -z "$db" ]; then
        # Имя по умолчанию: data/rustspace.db, у старых версий — data/backspace.db.
        db=data/rustspace.db
        [ -f "$OLD_CWD/$db" ] || db=data/backspace.db
    fi
    up=$(carried UPLOAD_DIR); up=${up:-data/uploads}
    case "$db" in /*) SRC_DB=$db ;; *) SRC_DB="$OLD_CWD/$db" ;; esac
    case "$up" in /*) SRC_UPLOADS=$up ;; *) SRC_UPLOADS="$OLD_CWD/$up" ;; esac
    [ -n "$OLD_DB_URL" ] && SRC_DB=""
fi

# Прежняя установка на SQLite: при переходе на PostgreSQL переносится и её база.
# После переноса база переименовывается в *.migrated, так что найденная здесь —
# ещё не перенесённая (в том числе если прошлый запуск оборвался).
OWN_SQLITE=""
if [ "$UPDATE" = 1 ]; then
    for f in "$(carried DB_PATH)" "$DB_FILE" "$OLD_DATA/data/backspace.db"; do
        [ -n "$f" ] && [ -f "$f" ] && { OWN_SQLITE=$f; break; }
    done
fi

# Адрес машины в локальной сети — для подсказок про NPM и роутер.
LAN_IP=$(ip -4 route get 1.1.1.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -n 1)
if [ -z "$LAN_IP" ]; then
    for a in $(hostname -I 2>/dev/null) $(hostname -i 2>/dev/null); do
        case "$a" in 127.*|*:*) ;; *) LAN_IP=$a; break ;; esac
    done
fi

# ─── План ───────────────────────────────────────────────────────────────────

if [ -n "$BINARY" ]; then
    SERVER_FROM="готовый файл $BINARY"
elif [ -n "$RELEASE" ]; then
    SERVER_FROM="релиз $RELEASE из github.com/$GITHUB_REPO"
else
    SERVER_FROM="сборка из $REPO"
fi
case "$DB_CHOICE" in
    local) DB_TEXT="PostgreSQL на этой машине (база $APP)" ;;
    external) DB_TEXT="внешний PostgreSQL" ;;
    *) DB_TEXT="SQLite, $DB_FILE" ;;
esac
MIGRATE_TEXT="нет"
if [ "$DB_CHOICE" != sqlite ] && [ -n "$SRC_DB$OWN_SQLITE" ]; then
    MIGRATE_TEXT="${SRC_DB:-$OWN_SQLITE} → PostgreSQL (если он ещё пуст)"
elif [ -n "$SRC_DB" ]; then
    MIGRATE_TEXT="копия $SRC_DB"
fi
if [ -n "$WEB_DIST" ] && [ -n "$WITH_WEB" ]; then
    die "--web-dist нельзя вместе с --web или --no-web"
fi
if [ "$WITH_WEB" = 1 ] && [ -n "$BINARY" ]; then
    die "с --binary веб-версию не скачать и не собрать: укажите собранную через --web-dist"
fi
OLD_WEB=$(carried BS_WEB_DIST)
if [ -z "$WITH_WEB" ] && [ -z "$WEB_DIST" ] && [ -z "$OLD_WEB" ] && [ "$UPDATE" != 1 ] \
    && [ "$ASSUME_YES" != 1 ]; then
    if confirm "Включить веб-версию Rustspace (браузер, десктоп Backspace)? Можно и позже — запуском с --web"; then
        WITH_WEB=1
    else
        WITH_WEB=0
    fi
fi
if [ -n "$WEB_DIST" ]; then
    WEB_TEXT="готовая сборка из $WEB_DIST"
elif [ "$WITH_WEB" = 1 ]; then
    WEB_TEXT="Rustspace в браузере (вместе с сервером)"
elif [ "$WITH_WEB" = 0 ] && [ -n "$OLD_WEB" ]; then
    WEB_TEXT="убирается"
elif [ -n "$OLD_WEB" ]; then
    WEB_TEXT="остаётся ($OLD_WEB)"
else
    WEB_TEXT="нет (включить — --web)"
fi
if [ "$UPDATE" = 1 ]; then
    MODE_TEXT="обновление"
    [ "$LEGACY" = 1 ] && MODE_TEXT="обновление и переименование backspace → rustspace"
else
    MODE_TEXT="новая установка"
fi

cat <<EOF

  Режим:          $MODE_TEXT
  Система:        ${PRETTY_NAME:-$OS} ($ARCH), службы: $INIT
  Домен:          https://$DOMAIN  ->  http://${LAN_IP:-эта-машина}:$PORT
  Голос, стримы:  $([ "$WITH_LIVEKIT" = 1 ] && echo "LiveKit $LIVEKIT_VERSION, https://$LIVEKIT_HOST -> :7880" || echo "выключены")
  Сервер:         $SERVER_FROM
  База:           $DB_TEXT
  Перенос базы:   $MIGRATE_TEXT
  Веб-версия:     $WEB_TEXT
  Файлы:          $DATA/data/uploads$([ -n "$SRC_UPLOADS" ] && echo " (копия из $SRC_UPLOADS)")
  Прежний процесс: $([ -n "$OLD_PID" ] && echo "pid $OLD_PID будет остановлен" || echo "нет")

EOF
confirm "Продолжить?" || die "остановлено"

# ─── Пакеты ─────────────────────────────────────────────────────────────────

say "Ставлю пакеты"
BUILDING=0
[ -z "$BINARY" ] && [ -z "$RELEASE" ] && BUILDING=1
if [ "$OS" = alpine ]; then
    apk add --no-cache ca-certificates curl tar ffmpeg iproute2 >/dev/null
    [ "$BUILDING" = 0 ] || apk add --no-cache build-base >/dev/null
    if [ "$DB_CHOICE" = local ] && ! command -v psql >/dev/null 2>&1; then
        installed=0
        for v in 18 17 16; do
            if apk add --no-cache "postgresql$v" "postgresql$v-client" "postgresql$v-openrc" >/dev/null 2>&1; then
                installed=1
                break
            fi
        done
        [ "$installed" = 1 ] || die "не удалось поставить PostgreSQL (apk add postgresql18/17/16)"
    fi
else
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq ca-certificates curl tar ffmpeg iproute2 >/dev/null
    [ "$BUILDING" = 0 ] || apt-get install -y -qq build-essential pkg-config >/dev/null
    [ "$DB_CHOICE" != local ] || apt-get install -y -qq postgresql >/dev/null
fi

# ─── PostgreSQL ─────────────────────────────────────────────────────────────

pg_ready() { as_postgres "psql -qAtX -c 'SELECT 1'" >/dev/null 2>&1; }

start_postgres() {
    pg_ready && return 0
    if [ "$INIT" = systemd ]; then
        systemctl enable postgresql >/dev/null 2>&1 || true
        systemctl start postgresql
    elif [ "$INIT" = openrc ]; then
        rc-update add postgresql default >/dev/null 2>&1 || true
        # Первый запуск под OpenRC сам создаёт кластер.
        rc-service postgresql start >/dev/null
    elif [ "$OS" = debian ]; then
        # Без системы инициализации (контейнер) кластеры Debian запускаются так.
        for cluster in /etc/postgresql/*/main; do
            [ -d "$cluster" ] || continue
            pg_ctlcluster "$(basename "$(dirname "$cluster")")" main start 2>/dev/null || true
        done
    else
        die "PostgreSQL на Alpine без OpenRC скрипт не запускает: поднимите его сами и укажите --database-url, или --sqlite"
    fi
    i=0
    until pg_ready; do
        i=$((i + 1))
        [ $i -le 30 ] || die "PostgreSQL не запустился за 30 с"
        sleep 1
    done
}

if [ "$DB_CHOICE" = local ]; then
    say "Готовлю PostgreSQL: пользователь и база $APP"
    start_postgres
    if [ "$(pg_sql "SELECT 1 FROM pg_roles WHERE rolname = '$APP'")" = 1 ]; then
        pg_sql "ALTER ROLE $APP WITH LOGIN PASSWORD '$DB_PASSWORD'"
    else
        pg_sql "CREATE ROLE $APP WITH LOGIN PASSWORD '$DB_PASSWORD'"
    fi
    if [ "$(pg_sql "SELECT 1 FROM pg_database WHERE datname = '$APP'")" != 1 ]; then
        pg_sql "CREATE DATABASE $APP OWNER $APP ENCODING 'UTF8' TEMPLATE template0"
    fi
fi

# Есть ли в локальном PostgreSQL пользователи Rustspace.
pg_has_users() {
    [ "$DB_CHOICE" = local ] || return 1
    [ "$(as_postgres "psql -qAtX -d $APP -c 'SELECT 1 FROM users LIMIT 1'" 2>/dev/null)" = 1 ]
}

# ─── Сервер ─────────────────────────────────────────────────────────────────

if [ -n "$BINARY" ]; then
    cp "$BINARY" "$NEW_BIN"
elif [ -n "$RELEASE" ]; then
    asset="rustspace-server-linux-$RUST_ARCH.tar.gz"
    say "Скачиваю $(release_url "$asset")"
    tmp=$(mktemp -d)
    fetch_release "$asset" "$tmp"
    tar xzf "$tmp/$asset" -C "$tmp" rustspace-server
    cp "$tmp/rustspace-server" "$NEW_BIN"
    rm -rf "$tmp"
else
    say "Собираю rustspace-server (первый раз — 5–20 минут)"
    # Каталог сборки со старым именем переезжает: не собирать всё заново.
    if [ ! -d "$BUILD" ] && [ -d "$OLD_BUILD" ]; then
        mv "$OLD_BUILD" "$BUILD"
    fi
    export RUSTUP_HOME="$BUILD/rustup" CARGO_HOME="$BUILD/cargo" CARGO_TARGET_DIR="$BUILD/target"
    TOOLCHAIN=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$REPO/rust-toolchain.toml")
    if [ ! -x "$CARGO_HOME/bin/rustup" ]; then
        mkdir -p "$BUILD"
        curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain none >/dev/null
    fi
    "$CARGO_HOME/bin/rustup" toolchain install "$TOOLCHAIN" --profile minimal >/dev/null
    # Свой набор целей из rust-toolchain.toml (Windows и т. п.) серверу не нужен.
    export RUSTUP_TOOLCHAIN="$TOOLCHAIN"
    (cd "$REPO" && "$CARGO_HOME/bin/cargo" build --release --locked -p bs-server ${JOBS:+--jobs "$JOBS"})
    cp "$CARGO_TARGET_DIR/release/rustspace-server" "$NEW_BIN"
fi
chmod 755 "$NEW_BIN"

# ─── LiveKit ────────────────────────────────────────────────────────────────

if [ "$WITH_LIVEKIT" = 1 ]; then
    current=""
    [ -x "$LK_BIN" ] && current=$("$LK_BIN" --version 2>/dev/null | sed -n 's/.*version \([0-9.]*\).*/\1/p')
    if [ "$current" != "$LIVEKIT_VERSION" ]; then
        say "Скачиваю LiveKit $LIVEKIT_VERSION"
        archive="livekit_${LIVEKIT_VERSION}_linux_${ARCH}.tar.gz"
        base="https://github.com/livekit/livekit/releases/download/v$LIVEKIT_VERSION"
        tmp=$(mktemp -d)
        curl -sSfL -o "$tmp/$archive" "$base/$archive" || die "не удалось скачать $base/$archive"
        if [ "$LIVEKIT_VERSION" = 1.13.11 ]; then
            expected=$(pinned_livekit_sha "$ARCH")
        else
            expected=$(curl -sSfL "$base/checksums.txt" | sed -n "s/^\([0-9a-f]*\)  $archive\$/\1/p")
        fi
        actual=$(sha256sum "$tmp/$archive" | cut -d' ' -f1)
        [ -n "$expected" ] && [ "$actual" = "$expected" ] || die "хэш архива LiveKit не совпал ($actual)"
        tar xzf "$tmp/$archive" -C "$tmp" livekit-server
        install -m 755 "$tmp/livekit-server" "$LK_BIN"
        rm -rf "$tmp"
    fi
fi

# ─── Остановка служб ────────────────────────────────────────────────────────

stop_service() {
    if [ "$INIT" = systemd ]; then
        systemctl stop "$1" 2>/dev/null || true
    elif [ "$INIT" = openrc ]; then
        rc-service "$1" stop >/dev/null 2>&1 || true
    fi
}

# Службу со старым именем — остановить и убрать совсем.
remove_service() {
    stop_service "$1"
    if [ "$INIT" = systemd ]; then
        systemctl disable "$1" >/dev/null 2>&1 || true
        rm -f "/etc/systemd/system/$1.service"
        systemctl daemon-reload
    elif [ "$INIT" = openrc ]; then
        rc-update del "$1" default >/dev/null 2>&1 || true
        rm -f "/etc/init.d/$1"
    fi
}

stop_service "$APP"
[ "$LEGACY" = 0 ] || remove_service "$OLD_NAME"

if [ -n "$OLD_PID" ] && kill -0 "$OLD_PID" 2>/dev/null; then
    say "Останавливаю прежний сервер (pid $OLD_PID)"
    kill -TERM "$OLD_PID"
    i=0
    while kill -0 "$OLD_PID" 2>/dev/null; do
        i=$((i + 1))
        [ $i -le 30 ] || die "прежний процесс не остановился за 30 с"
        sleep 1
    done
fi

# ─── Резервная копия ────────────────────────────────────────────────────────

STAMP=$(date +%Y%m%d-%H%M%S)
mkdir -p "$BACKUPS"
chown root:root "$BACKUPS"
chmod 700 "$BACKUPS"
if pg_has_users; then
    say "Резервная копия базы: $BACKUPS/$APP-$STAMP.dump"
    as_postgres "pg_dump -Fc $APP" > "$BACKUPS/$APP-$STAMP.dump"
    chmod 600 "$BACKUPS/$APP-$STAMP.dump"
elif [ -n "$OWN_SQLITE" ] || { [ "$DB_CHOICE" = sqlite ] && [ -f "$DB_FILE" ]; }; then
    # SQLite: каталог с базой и её журналом (-wal/-shm) — без журнала копия
    # неполная. Сервер в этот момент остановлен.
    source_db=${OWN_SQLITE:-$DB_FILE}
    say "Резервная копия базы: $BACKUPS/$APP-$STAMP.sqlite/"
    mkdir -m 700 "$BACKUPS/$APP-$STAMP.sqlite"
    for suffix in "" -wal -shm; do
        if [ -f "$source_db$suffix" ]; then
            cp "$source_db$suffix" "$BACKUPS/$APP-$STAMP.sqlite/$APP.db$suffix"
        fi
    done
fi
# Хранятся пять последних копий; сделанные вручную (…-manual) — пока их
# не удалит администратор.
# shellcheck disable=SC2010
ls -1t "$BACKUPS" 2>/dev/null | grep -v -- '-manual$' | tail -n +6 \
    | while read -r old; do rm -rf "${BACKUPS:?}/$old"; done

# ─── Пользователь, каталоги, перенос со старых имён ─────────────────────────

say "Готовлю пользователя, каталоги и настройки"
if ! id "$APP" >/dev/null 2>&1; then
    if [ "$OS" = alpine ]; then
        addgroup -S "$APP"
        adduser -S -D -H -h "$DATA" -s /sbin/nologin -G "$APP" "$APP"
    else
        useradd --system --home-dir "$DATA" --shell /usr/sbin/nologin --user-group "$APP"
    fi
fi

if [ "$LEGACY" = 1 ] && [ -d "$OLD_DATA" ] && [ ! -e "$DATA" ]; then
    say "Переношу $OLD_DATA → $DATA"
    mv "$OLD_DATA" "$DATA"
    for suffix in "" -wal -shm; do
        if [ -f "$DATA/data/backspace.db$suffix" ]; then
            mv "$DATA/data/backspace.db$suffix" "$DB_FILE$suffix"
        fi
    done
    if [ "$OWN_SQLITE" = "$OLD_DATA/data/backspace.db" ] || [ "$OWN_SQLITE" = "$(carried DB_PATH)" ]; then
        [ -f "$DB_FILE" ] && OWN_SQLITE=$DB_FILE
    fi
    if [ -d "$OLD_LOG_DIR" ] && [ ! -e "$LOG_DIR" ]; then
        mv "$OLD_LOG_DIR" "$LOG_DIR"
    fi
fi
mkdir -p "$ETC" "$DATA/data/uploads" "$LOG_DIR"
# Файлы, перенесённые администратором на другой диск (из клиента), остаются там.
UPLOADS=$DATA/data/uploads
prev_uploads=$(carried UPLOAD_DIR)
case "$prev_uploads" in
    /*/rustspace-uploads) [ -d "$prev_uploads" ] && UPLOADS=$prev_uploads ;;
esac
chown -R "$APP:$APP" "$DATA/data" "$LOG_DIR"
chown "$APP:$APP" "$DATA"
chmod 750 "$DATA"

# Копия данных сервера, запущенного вручную, или из --data-from.
if [ -n "$SRC_UPLOADS" ] && [ -d "$SRC_UPLOADS" ] && [ "$SRC_UPLOADS" != "$UPLOADS" ]; then
    if [ -n "$(ls -A "$DATA/data/uploads" 2>/dev/null)" ]; then
        warn "в $DATA/data/uploads уже есть файлы — копирование $SRC_UPLOADS пропущено"
    else
        say "Копирую файлы $SRC_UPLOADS"
        cp -Rp "$SRC_UPLOADS/." "$DATA/data/uploads/"
        chown -R "$APP:$APP" "$DATA/data/uploads"
    fi
fi
# Чужая база SQLite сначала копируется к себе: и для работы на SQLite, и для
# переноса в PostgreSQL (так оборванный перенос продолжится при повторном запуске).
if [ -n "$SRC_DB" ]; then
    if [ -f "$DB_FILE" ]; then
        warn "в $DATA/data уже есть база — беру её, а $SRC_DB не трогаю"
        [ "$DB_CHOICE" = sqlite ] || OWN_SQLITE=$DB_FILE
    else
        [ -f "$SRC_DB" ] || die "нет базы $SRC_DB"
        say "Копирую базу $SRC_DB"
        for f in "$SRC_DB" "$SRC_DB-wal" "$SRC_DB-shm"; do
            if [ -f "$f" ]; then
                cp -p "$f" "$DB_FILE${f#"$SRC_DB"}"
            fi
        done
        chown "$APP:$APP" "$DB_FILE"*
        [ "$DB_CHOICE" = sqlite ] || OWN_SQLITE=$DB_FILE
    fi
fi

# ─── Настройки ──────────────────────────────────────────────────────────────

if [ -n "$SOURCE_URL" ]; then
    case "$SOURCE_URL" in http://*|https://*) ;; *) die "--source-url должен начинаться с https://" ;; esac
    printf 'BACKSPACE_SOURCE_URL=%s\n' "$SOURCE_URL" >> "$CARRY"
fi

if [ -n "$TELEGRAM_APP" ]; then
    tg_id=${TELEGRAM_APP%%:*}
    tg_hash=${TELEGRAM_APP#*:}
    case "$tg_id" in ''|*[!0-9]*) die "--telegram-app: ID — число, например 1234567:0123abcd…" ;; esac
    printf '%s' "$tg_hash" | grep -Eq '^[0-9a-fA-F]{32}$' \
        || die "--telegram-app: HASH — 32 шестнадцатеричных символа"
    printf 'TELEGRAM_API_ID=%s\nTELEGRAM_API_HASH=%s\n' "$tg_id" "$tg_hash" >> "$CARRY"
fi

# Веб-клиент копируется к данным: служба не видит домашние каталоги.
WEB_DIR=$(carried BS_WEB_DIST)
case "$WEB_DIR" in "$OLD_DATA"/*) WEB_DIR="$DATA/${WEB_DIR#"$OLD_DATA"/}" ;; esac
if [ -n "$WEB_DIST" ]; then
    say "Копирую веб-клиент из $WEB_DIST"
    rm -rf "$DATA/web"
    cp -R "$WEB_DIST" "$DATA/web"
    chown -R "root:$APP" "$DATA/web"
    chmod -R g+rX,o-rwx "$DATA/web"
    WEB_DIR="$DATA/web"
elif [ "$WITH_WEB" = 0 ]; then
    [ -z "$WEB_DIR" ] || say "Убираю веб-версию"
    [ "$WEB_DIR" != "$DATA/web" ] || rm -rf "$DATA/web"
    WEB_DIR=""
else
    # Веб-версия, поставленная скриптом, обновляется вместе с сервером;
    # веб-клиент оригинала (его ставил setup.sh до 0.4.0) заменяется ею.
    if [ "$WITH_WEB" = 1 ] || { [ "$WEB_DIR" = "$DATA/web" ] \
        && { [ -f "$DATA/web/.rustspace-web" ] || [ -f "$DATA/web/.backspace-web" ]; }; }; then
        install_web
        WEB_DIR="$DATA/web"
    fi
fi

NEW_ENV=$(mktemp)
{
    echo "# Настройки rustspace-server. Записаны packaging/server/setup.sh;"
    echo "# правьте и перезапускайте службу. Описание переменных — README, «Configuration»."
    echo "DOMAIN='$DOMAIN'"
    echo "HOST='0.0.0.0'"
    echo "PORT='$PORT'"
    echo "TRUSTED_PROXY_HOPS='$PROXY_HOPS'"
    echo "JWT_SECRET='$JWT_SECRET'"
    if [ -n "$DATABASE_URL" ]; then
        echo "DATABASE_URL='$DATABASE_URL'"
    else
        echo "# setup: --sqlite"
        echo "DB_PATH='$DB_FILE'"
    fi
    echo "UPLOAD_DIR='$UPLOADS'"
    # Каталог заявок на обновление из клиента (см. rustspace-update.sh).
    [ "$WITH_UPDATER" = 0 ] || [ -z "$RUST_ARCH" ] || echo "BS_UPDATER_DIR='$DATA/updater'"
    [ -z "$WEB_DIR" ] || echo "BS_WEB_DIST='$WEB_DIR'"
    if [ "$WITH_LIVEKIT" = 1 ]; then
        echo "LIVEKIT_URL='wss://$LIVEKIT_HOST'"
        echo "LIVEKIT_API_KEY='$LK_KEY'"
        echo "LIVEKIT_API_SECRET='$LK_SECRET'"
    fi
    for v in $KNOWN_VARS; do
        is_managed "$v" && continue
        value=$(carried "$v")
        [ -n "$value" ] || continue
        check_value "$v" "$value"
        echo "$v='$value'"
    done
} > "$NEW_ENV"
install -m 600 -o root -g root "$NEW_ENV" "$ENV_FILE"
rm -f "$NEW_ENV"

if [ "$WITH_LIVEKIT" = 1 ]; then
    if ! id livekit >/dev/null 2>&1; then
        if [ "$OS" = alpine ]; then
            addgroup -S livekit
            adduser -S -D -H -s /sbin/nologin -G livekit livekit
        else
            useradd --system --no-create-home --shell /usr/sbin/nologin --user-group livekit
        fi
    fi
    NEW_LK=$(mktemp)
    cat > "$NEW_LK" <<EOF
# LiveKit для Rustspace. Записан packaging/server/setup.sh.
# Подключение клиентов: порт 7880 за обратным прокси (wss://$LIVEKIT_HOST).
# Медиа: TCP 7881 и UDP 7882 пробрасываются на эту машину напрямую.
port: 7880
rtc:
  tcp_port: 7881
  udp_port: 7882
  # внешний адрес — через STUN; за домашним роутером без «петли» самопроверка
  # не проходит, поэтому она выключена; клиенты из этой же сети подключаются
  # по внутреннему адресу
  use_external_ip: true
  skip_external_ip_validation: true
  advertise_internal_ip: true
keys:
  $LK_KEY: $LK_SECRET
logging:
  level: info
EOF
    install -m 640 -o root -g livekit "$NEW_LK" "$LK_FILE"
    rm -f "$NEW_LK"
fi

# ─── Файл сервера и перенос SQLite → PostgreSQL ─────────────────────────────

install -m 755 "$NEW_BIN" "$BIN"

SQLITE_SOURCE=""
[ "$DB_CHOICE" = sqlite ] || SQLITE_SOURCE=$OWN_SQLITE
if [ -n "$SQLITE_SOURCE" ]; then
    [ -f "$SQLITE_SOURCE" ] || die "нет базы $SQLITE_SOURCE"
    if pg_has_users; then
        warn "в PostgreSQL уже есть пользователи — перенос $SQLITE_SOURCE пропущен"
    else
        say "Переношу данные $SQLITE_SOURCE → PostgreSQL"
        # Перенос читает копию: исходная база остаётся нетронутой.
        work=$(mktemp -d)
        for f in "$SQLITE_SOURCE" "$SQLITE_SOURCE-wal" "$SQLITE_SOURCE-shm"; do
            if [ -f "$f" ]; then
                cp -p "$f" "$work/source.db${f#"$SQLITE_SOURCE"}"
            fi
        done
        if with_server_env "$BIN" import-sqlite "$work/source.db"; then
            rm -rf "$work"
            chown -R "$APP:$APP" "$DATA/data"
            # Прежняя база больше не используется: переименовать, но не удалять,
            # пока администратор не убедится, что всё на месте.
            case "$SQLITE_SOURCE" in
                "$DATA"/*)
                    for suffix in "" -wal -shm; do
                        if [ -f "$SQLITE_SOURCE$suffix" ]; then
                            mv "$SQLITE_SOURCE$suffix" "$SQLITE_SOURCE$suffix.migrated"
                        fi
                    done
                    say "Прежняя база сохранена как $SQLITE_SOURCE.migrated" ;;
            esac
        else
            rm -rf "$work"
            die "перенос не удался; PostgreSQL не изменён (перенос идёт одной транзакцией), прежняя база цела"
        fi
    fi
fi

# ─── Службы ─────────────────────────────────────────────────────────────────

say "Настраиваю службы ($INIT)"
PG_UNIT=""
[ "$DB_CHOICE" = local ] && PG_UNIT=" postgresql.service"
if [ "$INIT" = systemd ]; then
    cat > "/etc/systemd/system/$APP.service" <<EOF
[Unit]
Description=Rustspace server (bs-server)
After=network-online.target$PG_UNIT
Wants=network-online.target$PG_UNIT

[Service]
User=$APP
Group=$APP
EnvironmentFile=$ENV_FILE
WorkingDirectory=$DATA
ExecStart=$BIN
Restart=on-failure
RestartSec=3
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ProtectHome=true

[Install]
WantedBy=multi-user.target
EOF
    if [ "$WITH_LIVEKIT" = 1 ]; then
        cat > /etc/systemd/system/livekit.service <<EOF
[Unit]
Description=LiveKit (voice and video for Rustspace)
After=network-online.target
Wants=network-online.target

[Service]
User=livekit
Group=livekit
ExecStart=$LK_BIN --config $LK_FILE
Restart=on-failure
RestartSec=3
LimitNOFILE=65536
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ProtectHome=true

[Install]
WantedBy=multi-user.target
EOF
    fi
    systemctl daemon-reload
    if [ "$WITH_LIVEKIT" = 1 ]; then
        systemctl enable livekit >/dev/null 2>&1
        systemctl restart livekit
    fi
    systemctl enable "$APP" >/dev/null 2>&1
    systemctl restart "$APP"
elif [ "$INIT" = openrc ]; then
    cat > "/etc/init.d/$APP" <<EOF
#!/sbin/openrc-run
name="$APP"
description="Rustspace server (bs-server)"
command="$BIN"
command_user="$APP:$APP"
directory="$DATA"
command_background=true
pidfile="/run/\${RC_SVCNAME}.pid"
output_log="$LOG_DIR/$APP.log"
error_log="$LOG_DIR/$APP.log"

depend() {
    need net
    after livekit postgresql
}

start_pre() {
    set -a
    . $ENV_FILE
    set +a
}
EOF
    chmod 755 "/etc/init.d/$APP"
    if [ "$WITH_LIVEKIT" = 1 ]; then
        cat > /etc/init.d/livekit <<EOF
#!/sbin/openrc-run
name="livekit"
description="LiveKit (voice and video for Rustspace)"
command="$LK_BIN"
command_args="--config $LK_FILE"
command_user="livekit:livekit"
command_background=true
pidfile="/run/\${RC_SVCNAME}.pid"
output_log="$LOG_DIR/livekit.log"
error_log="$LOG_DIR/livekit.log"
rc_ulimit="-n 65536"

depend() {
    need net
}
EOF
        chmod 755 /etc/init.d/livekit
        touch "$LOG_DIR/livekit.log" && chown livekit:livekit "$LOG_DIR/livekit.log"
        rc-update add livekit default >/dev/null
        rc-service livekit restart >/dev/null
    fi
    touch "$LOG_DIR/$APP.log" && chown "$APP:$APP" "$LOG_DIR/$APP.log"
    rc-update add "$APP" default >/dev/null
    rc-service "$APP" restart >/dev/null
else
    warn "systemd и OpenRC не найдены (контейнер?): службы не заведены. Запуск вручную:"
    # Настройки читает root (в них секреты), а сервер работает от $APP.
    warn "  sh -c 'set -a; . $ENV_FILE; set +a; cd $DATA && exec su -s /bin/sh $APP -c $BIN'"
    [ "$WITH_LIVEKIT" = 0 ] || warn "  su -s /bin/sh livekit -c '$LK_BIN --config $LK_FILE'"
fi

# ─── Обновление из клиента ──────────────────────────────────────────────────
# Администратор нажимает «Обновить сервер» — сервер (без root) кладёт заявку
# в $DATA/updater, а этот помощник (root) ставит выпуск из $GITHUB_REPO,
# сверив контрольную сумму, с резервной копией базы и откатом.

UPDATER=/usr/local/lib/rustspace/rustspace-update.sh
if [ "$WITH_UPDATER" = 1 ] && [ -n "$RUST_ARCH" ] && { [ "$INIT" = systemd ] || [ "$INIT" = openrc ]; }; then
    say "Ставлю помощника обновления из клиента"
    mkdir -p /usr/local/lib/rustspace
    if [ -f "$SCRIPT_DIR/rustspace-update.sh" ]; then
        install -m 755 -o root -g root "$SCRIPT_DIR/rustspace-update.sh" "$UPDATER"
    else
        ref=main
        [ -z "$RELEASE" ] || [ "$RELEASE" = latest ] || ref="v$RELEASE"
        curl -sSfL -o "$UPDATER.new" \
            "https://raw.githubusercontent.com/$GITHUB_REPO/$ref/packaging/server/rustspace-update.sh" \
            && install -m 755 -o root -g root "$UPDATER.new" "$UPDATER"
        rm -f "$UPDATER.new"
    fi
    install -d -m 750 -o "$APP" -g "$APP" "$DATA/updater"
    cat > "$ETC/updater.conf" <<EOF
APP='$APP'
BIN='$BIN'
ENV_FILE='$ENV_FILE'
UPDATER_DIR='$DATA/updater'
GITHUB_REPO='$GITHUB_REPO'
RUST_ARCH='$RUST_ARCH'
INIT='$INIT'
PORT='$PORT'
BACKUPS='$BACKUPS'
DB_CHOICE='$DB_CHOICE'
EOF
    chmod 600 "$ETC/updater.conf"
    if [ "$INIT" = systemd ]; then
        cat > "/etc/systemd/system/$APP-update.service" <<EOF
[Unit]
Description=Rustspace: update requested from the client

[Service]
Type=oneshot
ExecStart=$UPDATER
EOF
        cat > "/etc/systemd/system/$APP-update.path" <<EOF
[Unit]
Description=Rustspace: watch for update requests

[Path]
PathExists=$DATA/updater/request.json
PathExists=$DATA/updater/job-request.json

[Install]
WantedBy=multi-user.target
EOF
        systemctl daemon-reload
        systemctl enable --now "$APP-update.path" >/dev/null 2>&1 || warn "не удалось включить $APP-update.path"
    else
        cat > "/etc/init.d/$APP-updater" <<EOF
#!/sbin/openrc-run
description="Rustspace: update requested from the client"
command="$UPDATER"
command_args="--watch"
command_background=true
pidfile="/run/$APP-updater.pid"
EOF
        chmod 755 "/etc/init.d/$APP-updater"
        rc-update add "$APP-updater" default >/dev/null
        rc-service "$APP-updater" restart >/dev/null
    fi
    touch "$DATA/updater/enabled"
    chown "$APP:$APP" "$DATA/updater/enabled"
    # Список резервных копий для клиента.
    "$UPDATER" --scan || warn "помощник не составил список резервных копий"
fi

# ─── Уборка старых имён ─────────────────────────────────────────────────────

if [ "$LEGACY" = 1 ]; then
    rm -f "$OLD_BIN"
    rm -rf "$OLD_ETC"
    if id "$OLD_NAME" >/dev/null 2>&1 && ! pgrep -u "$OLD_NAME" >/dev/null 2>&1; then
        if [ "$OS" = alpine ]; then
            deluser "$OLD_NAME" >/dev/null 2>&1 || true
            delgroup "$OLD_NAME" >/dev/null 2>&1 || true
        else
            userdel "$OLD_NAME" >/dev/null 2>&1 || true
            groupdel "$OLD_NAME" >/dev/null 2>&1 || true
        fi
    fi
    say "Старые имена убраны: $OLD_ETC, $OLD_BIN, служба и пользователь $OLD_NAME"
fi

# ─── Брандмауэр ─────────────────────────────────────────────────────────────

if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -q "Status: active"; then
    say "Открываю порты в ufw"
    ufw allow "$PORT/tcp" >/dev/null
    if [ "$WITH_LIVEKIT" = 1 ]; then
        ufw allow 7880/tcp >/dev/null
        ufw allow 7881/tcp >/dev/null
        ufw allow 7882/udp >/dev/null
    fi
fi

# ─── Проверка ───────────────────────────────────────────────────────────────

wait_for() {
    i=0
    while [ $i -lt 30 ]; do
        if out=$(curl -fsS --noproxy '*' --max-time 3 "$1" 2>/dev/null) && printf '%s' "$out" | grep -q "$2"; then
            return 0
        fi
        i=$((i + 1))
        sleep 1
    done
    return 1
}

journal_hint() {
    if [ "$INIT" = systemd ]; then echo "journalctl -u $1 -e"; else echo "$LOG_DIR/$1.log"; fi
}

OK=1
if [ "$INIT" != none ]; then
    if wait_for "http://127.0.0.1:$PORT/api/health" '"ok"'; then
        say "Сервер отвечает: http://127.0.0.1:$PORT/api/health"
    else
        OK=0
        warn "сервер не ответил за 30 с; журнал: $(journal_hint "$APP")"
    fi
    if [ "$WITH_LIVEKIT" = 1 ]; then
        if wait_for "http://127.0.0.1:7880/" 'OK'; then
            say "LiveKit отвечает: http://127.0.0.1:7880/"
        else
            OK=0
            warn "LiveKit не ответил за 30 с; журнал: $(journal_hint livekit)"
        fi
    fi
fi

if [ "$UPDATE" = 1 ] && [ "$LEGACY" = 0 ]; then
    cat <<EOF

────────────────────────────────────────────────────────────────────────
Обновление готово$([ "$OK" = 1 ] || echo " (с предупреждениями выше)").
База: $DB_TEXT; резервные копии — $BACKUPS (пять последних).
EOF
    exit 0
fi

IP=${LAN_IP:-ЭТА_МАШИНА}
cat <<EOF

────────────────────────────────────────────────────────────────────────
Готово$([ "$OK" = 1 ] || echo " (с предупреждениями выше)"). Осталось настроить снаружи.

1. Nginx Proxy Manager → Proxy Hosts → Add Proxy Host:
     Domain Names: $DOMAIN
     Scheme http, Forward Hostname/IP $IP, Port $PORT
     Websockets Support — ВКЛ.
     SSL: Request a new certificate, Force SSL, HTTP/2
EOF
if [ "$WITH_LIVEKIT" = 1 ]; then
    cat <<EOF

   И второй Proxy Host для голоса и стримов:
     Domain Names: $LIVEKIT_HOST
     Scheme http, Forward Hostname/IP $IP, Port 7880
     Websockets Support — ВКЛ.
     SSL: Request a new certificate, Force SSL, HTTP/2
   (У DuckDNS поддомены вида $LIVEKIT_HOST указывают на тот же адрес сами.)

2. Роутер → проброс портов на $IP:
     TCP 7881 и UDP 7882 — медиа LiveKit (мимо NPM)
   (80 и 443 — на машину с NPM, как и раньше.)
EOF
fi
cat <<EOF

3. Проверка снаружи:
     curl https://$DOMAIN/api/health          → {"status":"ok",…}
EOF
[ "$WITH_LIVEKIT" = 0 ] || echo "     curl https://$LIVEKIT_HOST/              → OK"
cat <<EOF
   Затем в клиенте: адрес https://$DOMAIN$([ -n "$SQLITE_SOURCE$SRC_DB" ] && echo " — прежние аккаунты на месте" || echo ", регистрация (первый — администратор)").

Настройки: $ENV_FILE$([ "$WITH_LIVEKIT" = 1 ] && echo ", $LK_FILE")
База: $DB_TEXT; резервные копии перед обновлениями — $BACKUPS
EOF
if [ "$INIT" = systemd ]; then
    echo "Управление: systemctl {status|restart|stop} $APP$([ "$WITH_LIVEKIT" = 1 ] && echo " livekit"); журнал: journalctl -u $APP -f"
elif [ "$INIT" = openrc ]; then
    echo "Управление: rc-service $APP {status|restart|stop}; журнал: $LOG_DIR/$APP.log"
fi
if [ -n "$WEB_DIR" ]; then
    echo "Веб-версия: https://$DOMAIN/ — браузер и десктоп-клиент Backspace подключаются к этому адресу."
else
    echo "Веб-версии нет: https://$DOMAIN/ показывает страницу «подключайтесь клиентом Rustspace» (включить — запуск с --web)."
fi
[ -n "$(carried BACKSPACE_SOURCE_URL)" ] || echo "Если вы меняли код сервера, укажите свой репозиторий: --source-url https://… (AGPL требует давать исходники запущенной версии)."
echo "Обновление: git pull && sudo sh $0 (или sudo sh $0 --release) — данные и секреты сохранятся."
