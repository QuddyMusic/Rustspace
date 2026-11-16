#!/bin/sh
# Установка и настройка сервера Backspace на Rust (bs-server) с нуля:
# Debian, Ubuntu или Alpine; systemd или OpenRC; голос и стримы через LiveKit.
#
#   sudo sh packaging/server/setup.sh --domain rustspace.duckdns.org
#
# Что делает (повторный запуск безопасен: секреты и данные сохраняются):
#   1. ставит пакеты (ffmpeg для превью видео, компилятор для сборки);
#   2. собирает backspace-server из этого репозитория (или берёт --binary);
#   3. скачивает LiveKit (сервер голоса, видео и стримов) и проверяет его хэш;
#   4. пишет настройки в /etc/backspace/ и заводит службы backspace и livekit;
#   5. если сервер уже был запущен вручную, переносит его JWT_SECRET, настройки,
#      базу и файлы, а старый процесс останавливает;
#   6. проверяет, что всё отвечает, и печатает, что настроить в NPM и на роутере.
#
# Схема: снаружи HTTPS делает обратный прокси (Nginx Proxy Manager, Caddy…):
#   https://DOMAIN     -> http://ЭТА_МАШИНА:3000  (Websockets Support вкл.)
#   https://lk.DOMAIN  -> http://ЭТА_МАШИНА:7880  (Websockets Support вкл.)
# Медиа LiveKit идёт мимо прокси: на роутере TCP 7881 и UDP 7882 на эту машину.

set -eu

# ─── Параметры ──────────────────────────────────────────────────────────────

DOMAIN=""
LIVEKIT_HOST=""
WITH_LIVEKIT=1
BINARY=""
PORT=3000
PROXY_HOPS=1
WEB_DIST=""
SOURCE_URL=""
DATA_FROM=""
ASSUME_YES=0
JOBS=""
LIVEKIT_VERSION="1.13.11"

# Хэши архивов LiveKit 1.13.11 (из checksums.txt релиза, сверены вручную):
# так подменённый архив не пройдёт, даже если подменён и checksums.txt.
pinned_livekit_sha() {
    case "$1" in
        amd64) echo 1e9ee90ca301e1842b2887fc77cb122759756a9c8aeb4334ee91de3325ec43fb ;;
        arm64) echo 2af6ce091d6b3c79d1677f8ff1bbb91a8007f40b2325c059b52d38383c0830b6 ;;
        armv7) echo 5f663a6498c3d81ef2dce55a3a1e7815aacfe78207e469abc0bd9c5b5ca5070b ;;
    esac
}

ETC=/etc/backspace
ENV_FILE=$ETC/backspace.env
LK_FILE=$ETC/livekit.yaml
DATA=/var/lib/backspace
BIN=/usr/local/bin/backspace-server
LK_BIN=/usr/local/bin/livekit-server
BUILD=/opt/backspace-build
LOG_DIR=/var/log/backspace

# Переменные, которые сервер читает; переносятся из прежних настроек.
KNOWN_VARS="BACKSPACE_COMMIT BACKSPACE_INSTALL_CHANNEL BACKSPACE_SOURCE_URL
BACKSPACE_UPDATE_CHECK BS_EXTRA_CA_CERTS BS_PASSWORD_HASH_COST BS_WEB_DIST
DATABASE_URL DB_PATH DIRECTORY_ENDPOINT DISABLE_FEDERATION_WORKERS DOMAIN
FEDERATION_ALLOW_PRIVATE_PEERS HOST JWT_EXPIRES_IN JWT_SECRET LIVEKIT_API_KEY
LIVEKIT_API_SECRET LIVEKIT_URL MAX_UPLOAD_SIZE PORT PUBLIC_ORIGIN
REGISTRATION_OPEN TRUSTED_PROXY_HOPS TUS_EXPIRATION_HOURS UPLOAD_DIR"
# Их скрипт задаёт сам.
MANAGED_VARS="DOMAIN HOST PORT TRUSTED_PROXY_HOPS DB_PATH UPLOAD_DIR JWT_SECRET
LIVEKIT_URL LIVEKIT_API_KEY LIVEKIT_API_SECRET"

usage() {
    cat <<'EOF'
Использование: sudo sh packaging/server/setup.sh --domain ДОМЕН [параметры]

  --domain ДОМЕН         домен инстанса (обязательно), например rustspace.duckdns.org
  --livekit-host ИМЯ     адрес LiveKit для клиентов (по умолчанию lk.ДОМЕН)
  --no-livekit           без голоса и стримов
  --binary ПУТЬ          готовый backspace-server вместо сборки из исходников
  --port ПОРТ            порт сервера (3000)
  --proxy-hops N         сколько прокси перед сервером (1; без прокси — 0)
  --web-dist КАТАЛОГ     собранный веб-клиент оригинала (packages/web/dist)
  --source-url URL       ваш репозиторий bs-rs: ссылка на исходный код и клиент
                         на главной странице (BACKSPACE_SOURCE_URL)
  --data-from КАТАЛОГ    перенести данные из каталога (backspace.db и uploads/)
  --livekit-version В    версия LiveKit (1.13.11)
  --jobs N               потоков сборки (меньше — меньше памяти)
  -y, --yes              не задавать вопросов
  -h, --help             эта справка
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --domain) DOMAIN="${2:-}"; shift 2 ;;
        --livekit-host) LIVEKIT_HOST="${2:-}"; shift 2 ;;
        --no-livekit) WITH_LIVEKIT=0; shift ;;
        --binary) BINARY="${2:-}"; shift 2 ;;
        --port) PORT="${2:-}"; shift 2 ;;
        --proxy-hops) PROXY_HOPS="${2:-}"; shift 2 ;;
        --web-dist) WEB_DIST="${2:-}"; shift 2 ;;
        --source-url) SOURCE_URL="${2:-}"; shift 2 ;;
        --data-from) DATA_FROM="${2:-}"; shift 2 ;;
        --livekit-version) LIVEKIT_VERSION="${2:-}"; shift 2 ;;
        --jobs) JOBS="${2:-}"; shift 2 ;;
        -y|--yes) ASSUME_YES=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Неизвестный параметр: $1" >&2; usage >&2; exit 2 ;;
    esac
done

# ─── Вспомогательное ────────────────────────────────────────────────────────

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31mОшибка:\033[0m %s\n' "$*" >&2; exit 1; }

confirm() {
    [ "$ASSUME_YES" = 1 ] && return 0
    printf '%s [y/N] ' "$1"
    read -r answer || answer=""
    case "$answer" in y|Y|yes|д|Д|да) return 0 ;; *) return 1 ;; esac
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

# ─── Проверки ───────────────────────────────────────────────────────────────

[ "$(id -u)" = 0 ] || die "запустите от root: sudo sh $0 --domain ДОМЕН"
[ -n "$DOMAIN" ] || { usage >&2; die "не указан --domain"; }
case "$DOMAIN" in
    http://*|https://*|*/*) die "--domain — только имя, без https:// и пути: ${DOMAIN#*://}" ;;
esac
case "$PORT" in ''|*[!0-9]*) die "--port должен быть числом" ;; esac
case "$PROXY_HOPS" in [0-4]) ;; *) die "--proxy-hops — от 0 до 4" ;; esac
[ -n "$LIVEKIT_HOST" ] || LIVEKIT_HOST="lk.$DOMAIN"
[ -z "$WEB_DIST" ] || [ -f "$WEB_DIST/index.html" ] || die "в $WEB_DIST нет index.html"
[ -z "$BINARY" ] || [ -x "$BINARY" ] || die "$BINARY — не исполняемый файл"
[ -z "$DATA_FROM" ] || [ -d "$DATA_FROM" ] || die "нет каталога $DATA_FROM"

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
    x86_64|amd64) ARCH=amd64 ;;
    aarch64|arm64) ARCH=arm64 ;;
    armv7l|armv7*) ARCH=armv7 ;;
    *) die "архитектура $(uname -m) не поддерживается" ;;
esac

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$SCRIPT_DIR/../.." && pwd)
if [ -z "$BINARY" ] && [ ! -f "$REPO/crates/bs-server/Cargo.toml" ]; then
    die "скрипт должен лежать в репозитории bs-rs (packaging/server/), или укажите --binary"
fi

# ─── Прежние настройки ──────────────────────────────────────────────────────

CARRY=$(mktemp)
trap 'rm -f "$CARRY"' EXIT
OLD_PID=""
OLD_CWD=""

if [ -f "$ENV_FILE" ]; then
    say "Найдены прежние настройки $ENV_FILE — секреты сохраняются"
    for v in $KNOWN_VARS; do
        value=$(file_value "$v" "$ENV_FILE")
        [ -n "$value" ] && printf '%s=%s\n' "$v" "$value" >> "$CARRY"
    done
else
    # Сервер, запущенный вручную: его настройки и данные — основа новой установки.
    # Ищем по файлу программы: имя процесса ядро обрезает до 15 знаков.
    for dir in /proc/[0-9]*; do
        exe=$(readlink "$dir/exe" 2>/dev/null) || continue
        case "$exe" in
            "$BIN"|"$BIN (deleted)") continue ;;
            */backspace-server|*"/backspace-server (deleted)") ;;
            *) continue ;;
        esac
        OLD_PID=${dir#/proc/}
        OLD_CWD=$(readlink "$dir/cwd" 2>/dev/null || echo /)
        break
    done
    if [ -n "$OLD_PID" ]; then
        say "Найден запущенный backspace-server (pid $OLD_PID, каталог $OLD_CWD): беру его настройки"
        for v in $KNOWN_VARS; do
            value=$(proc_value "$v" "$OLD_PID")
            [ -n "$value" ] && printf '%s=%s\n' "$v" "$value" >> "$CARRY"
        done
    fi
fi

carried() { sed -n "s/^$1=//p" "$CARRY" | tail -n 1; }

JWT_SECRET=$(carried JWT_SECRET)
if [ -z "$JWT_SECRET" ]; then
    if [ -n "$OLD_PID" ] || [ -n "$DATA_FROM" ]; then
        warn "у прежнего сервера не найден JWT_SECRET: с новым всем придётся войти заново,"
        warn "а двухфакторная защита у пользователей перестанет работать"
        confirm "Продолжить с новым JWT_SECRET?" || die "остановлено; задайте JWT_SECRET в $ENV_FILE и запустите снова"
    fi
    JWT_SECRET=$(random_hex 32)
fi
check_value JWT_SECRET "$JWT_SECRET"

LK_KEY=$(carried LIVEKIT_API_KEY)
LK_SECRET=$(carried LIVEKIT_API_SECRET)
OLD_LK_URL=$(carried LIVEKIT_URL)
# Прежние ключи LiveKit годятся, только если это он же (тот же адрес).
if [ "$WITH_LIVEKIT" = 1 ]; then
    if [ -z "$LK_KEY" ] || [ -z "$LK_SECRET" ] || [ "$OLD_LK_URL" != "wss://$LIVEKIT_HOST" ]; then
        LK_KEY="API$(random_hex 6)"
        LK_SECRET=$(random_hex 32)
    fi
    check_value LIVEKIT_API_KEY "$LK_KEY"
    check_value LIVEKIT_API_SECRET "$LK_SECRET"
fi

# Где лежали данные прежнего сервера.
OLD_DB=""
OLD_UPLOADS=""
if [ -n "$DATA_FROM" ]; then
    OLD_DB="$DATA_FROM/backspace.db"
    OLD_UPLOADS="$DATA_FROM/uploads"
elif [ -n "$OLD_PID" ]; then
    db=$(carried DB_PATH); db=${db:-data/backspace.db}
    up=$(carried UPLOAD_DIR); up=${up:-data/uploads}
    case "$db" in /*) OLD_DB=$db ;; *) OLD_DB="$OLD_CWD/$db" ;; esac
    case "$up" in /*) OLD_UPLOADS=$up ;; *) OLD_UPLOADS="$OLD_CWD/$up" ;; esac
fi
[ -n "$(carried DATABASE_URL)" ] && OLD_DB=""

# Адрес машины в локальной сети — для подсказок про NPM и роутер.
LAN_IP=$(ip -4 route get 1.1.1.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -n 1)
if [ -z "$LAN_IP" ]; then
    for a in $(hostname -I 2>/dev/null) $(hostname -i 2>/dev/null); do
        case "$a" in 127.*|*:*) ;; *) LAN_IP=$a; break ;; esac
    done
fi

# ─── План ───────────────────────────────────────────────────────────────────

cat <<EOF

  Система:        ${PRETTY_NAME:-$OS} ($ARCH), службы: $INIT
  Домен:          https://$DOMAIN  ->  http://${LAN_IP:-эта-машина}:$PORT
  Голос, стримы:  $([ "$WITH_LIVEKIT" = 1 ] && echo "LiveKit $LIVEKIT_VERSION, https://$LIVEKIT_HOST -> :7880" || echo "выключены")
  Сервер:         $([ -n "$BINARY" ] && echo "готовый файл $BINARY" || echo "сборка из $REPO")
  Данные:         $DATA$([ -n "$(carried DATABASE_URL)" ] && echo " (база — PostgreSQL)")
  Перенос:        $([ -n "$OLD_DB$OLD_UPLOADS" ] && echo "${OLD_DB:-база в PostgreSQL}, $OLD_UPLOADS" || echo "нет")
  Прежний процесс: $([ -n "$OLD_PID" ] && echo "pid $OLD_PID будет остановлен" || echo "нет")

EOF
confirm "Продолжить?" || die "остановлено"

# ─── Пакеты ─────────────────────────────────────────────────────────────────

say "Ставлю пакеты"
if [ "$OS" = alpine ]; then
    apk add --no-cache ca-certificates curl tar ffmpeg iproute2 >/dev/null
    [ -n "$BINARY" ] || apk add --no-cache build-base >/dev/null
else
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq ca-certificates curl tar ffmpeg iproute2 >/dev/null
    [ -n "$BINARY" ] || apt-get install -y -qq build-essential pkg-config >/dev/null
fi

# ─── Сервер ─────────────────────────────────────────────────────────────────

NEW_BIN=$(mktemp)
trap 'rm -f "$CARRY" "$NEW_BIN"' EXIT
if [ -n "$BINARY" ]; then
    cp "$BINARY" "$NEW_BIN"
else
    say "Собираю backspace-server (первый раз — 5–20 минут)"
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
    cp "$CARGO_TARGET_DIR/release/backspace-server" "$NEW_BIN"
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

# ─── Пользователи и каталоги ────────────────────────────────────────────────

say "Готовлю пользователя, каталоги и настройки"
if ! id backspace >/dev/null 2>&1; then
    if [ "$OS" = alpine ]; then
        addgroup -S backspace
        adduser -S -D -H -h "$DATA" -s /sbin/nologin -G backspace backspace
    else
        useradd --system --home-dir "$DATA" --shell /usr/sbin/nologin --user-group backspace
    fi
fi
mkdir -p "$ETC" "$DATA/data/uploads" "$LOG_DIR"
chown backspace:backspace "$DATA" "$DATA/data" "$DATA/data/uploads" "$LOG_DIR"
chmod 750 "$DATA"

# ─── Остановка прежнего процесса и перенос данных ───────────────────────────

stop_services() {
    if [ "$INIT" = systemd ]; then
        systemctl stop backspace 2>/dev/null || true
    elif [ "$INIT" = openrc ]; then
        rc-service backspace stop >/dev/null 2>&1 || true
    fi
}

if [ -n "$OLD_PID" ] && kill -0 "$OLD_PID" 2>/dev/null; then
    say "Останавливаю прежний backspace-server (pid $OLD_PID)"
    kill -TERM "$OLD_PID"
    i=0
    while kill -0 "$OLD_PID" 2>/dev/null; do
        i=$((i + 1))
        [ $i -le 30 ] || die "прежний процесс не остановился за 30 с"
        sleep 1
    done
fi

if [ -n "$OLD_DB$OLD_UPLOADS" ]; then
    if [ -f "$DATA/data/backspace.db" ]; then
        warn "в $DATA/data уже есть база — перенос пропущен, прежние данные не тронуты"
    else
        stop_services
        if [ -n "$OLD_DB" ]; then
            [ -f "$OLD_DB" ] || die "нет базы $OLD_DB"
            say "Копирую базу $OLD_DB"
            for f in "$OLD_DB" "$OLD_DB-wal" "$OLD_DB-shm"; do
                if [ -f "$f" ]; then
                    cp -p "$f" "$DATA/data/backspace.db${f#"$OLD_DB"}"
                fi
            done
        fi
        if [ -d "$OLD_UPLOADS" ]; then
            say "Копирую файлы $OLD_UPLOADS"
            cp -Rp "$OLD_UPLOADS/." "$DATA/data/uploads/"
        fi
        chown -R backspace:backspace "$DATA/data"
        say "Прежние данные остались на месте: удалите их сами, когда убедитесь, что всё работает"
    fi
fi

# ─── Настройки ──────────────────────────────────────────────────────────────

if [ -n "$SOURCE_URL" ]; then
    case "$SOURCE_URL" in http://*|https://*) ;; *) die "--source-url должен начинаться с https://" ;; esac
    printf 'BACKSPACE_SOURCE_URL=%s\n' "$SOURCE_URL" >> "$CARRY"
fi

# Веб-клиент копируется к данным: служба не видит домашние каталоги.
if [ -n "$WEB_DIST" ]; then
    say "Копирую веб-клиент из $WEB_DIST"
    rm -rf "$DATA/web"
    cp -R "$WEB_DIST" "$DATA/web"
    chown -R root:backspace "$DATA/web"
    chmod -R g+rX,o-rwx "$DATA/web"
    printf 'BS_WEB_DIST=%s\n' "$DATA/web" >> "$CARRY"
fi

NEW_ENV=$(mktemp)
{
    echo "# Настройки backspace-server. Записаны packaging/server/setup.sh;"
    echo "# правьте и перезапускайте службу. Описание переменных — README, «Configuration»."
    echo "DOMAIN='$DOMAIN'"
    echo "HOST='0.0.0.0'"
    echo "PORT='$PORT'"
    echo "TRUSTED_PROXY_HOPS='$PROXY_HOPS'"
    echo "JWT_SECRET='$JWT_SECRET'"
    echo "DB_PATH='$DATA/data/backspace.db'"
    echo "UPLOAD_DIR='$DATA/data/uploads'"
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
# LiveKit для Backspace. Записан packaging/server/setup.sh.
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

# ─── Установка файла сервера ────────────────────────────────────────────────

stop_services
install -m 755 "$NEW_BIN" "$BIN"

# ─── Службы ─────────────────────────────────────────────────────────────────

say "Настраиваю службы ($INIT)"
if [ "$INIT" = systemd ]; then
    cat > /etc/systemd/system/backspace.service <<EOF
[Unit]
Description=Backspace server (bs-server)
After=network-online.target
Wants=network-online.target

[Service]
User=backspace
Group=backspace
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
Description=LiveKit (voice and video for Backspace)
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
    systemctl enable backspace >/dev/null 2>&1
    systemctl restart backspace
elif [ "$INIT" = openrc ]; then
    cat > /etc/init.d/backspace <<EOF
#!/sbin/openrc-run
name="backspace"
description="Backspace server (bs-server)"
command="$BIN"
command_user="backspace:backspace"
directory="$DATA"
command_background=true
pidfile="/run/\${RC_SVCNAME}.pid"
output_log="$LOG_DIR/backspace.log"
error_log="$LOG_DIR/backspace.log"

depend() {
    need net
    after livekit
}

start_pre() {
    set -a
    . $ENV_FILE
    set +a
}
EOF
    chmod 755 /etc/init.d/backspace
    if [ "$WITH_LIVEKIT" = 1 ]; then
        cat > /etc/init.d/livekit <<EOF
#!/sbin/openrc-run
name="livekit"
description="LiveKit (voice and video for Backspace)"
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
    touch "$LOG_DIR/backspace.log" && chown backspace:backspace "$LOG_DIR/backspace.log"
    rc-update add backspace default >/dev/null
    rc-service backspace restart >/dev/null
else
    warn "systemd и OpenRC не найдены (контейнер?): службы не заведены. Запуск вручную:"
    # Настройки читает root (в них секреты), а сервер работает от backspace.
    warn "  sh -c 'set -a; . $ENV_FILE; set +a; cd $DATA && exec su -s /bin/sh backspace -c $BIN'"
    [ "$WITH_LIVEKIT" = 1 ] && warn "  su -s /bin/sh livekit -c '$LK_BIN --config $LK_FILE'"
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

OK=1
if [ "$INIT" != none ]; then
    if wait_for "http://127.0.0.1:$PORT/api/health" '"ok"'; then
        say "Сервер отвечает: http://127.0.0.1:$PORT/api/health"
    else
        OK=0
        warn "сервер не ответил за 30 с; журнал: $([ "$INIT" = systemd ] && echo "journalctl -u backspace -e" || echo "$LOG_DIR/backspace.log")"
    fi
    if [ "$WITH_LIVEKIT" = 1 ]; then
        if wait_for "http://127.0.0.1:7880/" 'OK'; then
            say "LiveKit отвечает: http://127.0.0.1:7880/"
        else
            OK=0
            warn "LiveKit не ответил за 30 с; журнал: $([ "$INIT" = systemd ] && echo "journalctl -u livekit -e" || echo "$LOG_DIR/livekit.log")"
        fi
    fi
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
[ "$WITH_LIVEKIT" = 1 ] && echo "     curl https://$LIVEKIT_HOST/              → OK"
cat <<EOF
   Затем в клиенте: адрес https://$DOMAIN$([ -n "$OLD_DB$OLD_UPLOADS" ] && echo " — прежние аккаунты на месте" || echo ", регистрация (первый — администратор)").

Настройки: $ENV_FILE$([ "$WITH_LIVEKIT" = 1 ] && echo ", $LK_FILE")
EOF
if [ "$INIT" = systemd ]; then
    echo "Управление: systemctl {status|restart|stop} backspace$([ "$WITH_LIVEKIT" = 1 ] && echo " livekit"); журнал: journalctl -u backspace -f"
elif [ "$INIT" = openrc ]; then
    echo "Управление: rc-service backspace {status|restart|stop}; журнал: $LOG_DIR/backspace.log"
fi
[ -z "$WEB_DIST" ] && echo "Веб-клиента нет: https://$DOMAIN/ показывает страницу «подключайтесь клиентом Rustspace» (свой веб-клиент — --web-dist)."
[ -z "$(carried BACKSPACE_SOURCE_URL)" ] && echo "Ссылка на ваш репозиторий для этой страницы: --source-url https://… (иначе ссылки на клиент там не будет)."
echo "Обновление: git pull и тот же запуск скрипта — данные и секреты сохранятся."
