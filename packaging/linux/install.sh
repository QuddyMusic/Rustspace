#!/bin/sh
# Rustspace для Linux: сборка из исходников и установка.
#
#   curl -fsSL https://raw.githubusercontent.com/QuddyMusic/Rustspace/main/packaging/linux/install.sh | sh
#
# Что делает:
#   1. ставит пакеты для сборки через менеджер пакетов системы (pacman, apt,
#      dnf, zypper) — спросив разрешения на sudo;
#   2. если нет cargo — предлагает поставить Rust через rustup;
#   3. скачивает исходники в ~/.local/src/rustspace (или обновляет их);
#   4. собирает клиент (cargo build --release) с голосом, камерой, треем;
#   5. кладёт программу, ярлык и значок:
#        ~/.local/bin/rustspace
#        ~/.local/share/applications/io.github.quddymusic.Rustspace.desktop
#        ~/.local/share/icons/hicolor/256x256/apps/rustspace.png
#      (с --system — в /usr/local через sudo).
# Обновить — запустить ещё раз: исходники подтянутся и пересоберутся.
#
# Параметры:
#   --ref ВЕТКА|ТЕГ|КОММИТ   что собирать (по умолчанию main)
#   --src КАТАЛОГ            где держать исходники
#   --system                 ставить в /usr/local для всех пользователей
#   --no-voice               без голоса и камеры (не нужен clang 21+)
#   --no-deps                не ставить системные пакеты
#   --yes                    не спрашивать (согласие на всё)
#   --uninstall              удалить программу (исходники и настройки остаются)
#
# Запуск из распакованного архива готовой сборки ставит файлы из него.
set -eu

REPO="${RUSTSPACE_REPO:-QuddyMusic/Rustspace}"
APP_ID="io.github.quddymusic.Rustspace"
REF="main"
SRC_DIR="$HOME/.local/src/rustspace"
SYSTEM=0
VOICE=1
DEPS=1
YES=0
UNINSTALL=0

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[33mВнимание:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[31mОшибка:\033[0m %s\n' "$*" >&2; exit 1; }

# Вопрос «да/нет». При `curl … | sh` стандартный ввод — сам скрипт, поэтому
# ответ читается с терминала.
ask() {
    [ "$YES" = 1 ] && return 0
    if [ -r /dev/tty ]; then
        printf '%s [Y/n] ' "$1" > /dev/tty
        read -r answer < /dev/tty || answer=""
        case "$answer" in [nN]*) return 1 ;; *) return 0 ;; esac
    fi
    warn "нет терминала для вопроса «$1» — считаю, что нет (запустите с --yes)"
    return 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --ref) REF="${2:?--ref: укажите ветку, тег или коммит}"; shift 2 ;;
        --src) SRC_DIR="${2:?--src: укажите каталог}"; shift 2 ;;
        --system) SYSTEM=1; shift ;;
        --no-voice) VOICE=0; shift ;;
        --no-deps) DEPS=0; shift ;;
        --yes|-y) YES=1; shift ;;
        --uninstall) UNINSTALL=1; shift ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) die "неизвестный параметр: $1" ;;
    esac
done

if [ "$SYSTEM" = 1 ]; then
    BIN_DIR=/usr/local/bin
    DATA_DIR=/usr/local/share
    SUDO=sudo
else
    BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
    DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
    SUDO=""
fi

if [ "$UNINSTALL" = 1 ]; then
    $SUDO rm -f "$BIN_DIR/rustspace" "$DATA_DIR/applications/$APP_ID.desktop" \
        "$DATA_DIR/icons/hicolor/256x256/apps/rustspace.png"
    say "Rustspace удалён. Настройки остались в ~/.config/rustspace, исходники — в $SRC_DIR"
    exit 0
fi

[ "$(id -u)" = 0 ] && die "запускайте без sudo: собирать от root не нужно (для пакетов скрипт сам спросит sudo)"

# --- Установка готовых файлов (общая для архива и сборки) -------------------
install_files() {
    bin="$1"; png="$2"; desktop="$3"
    $SUDO mkdir -p "$BIN_DIR" "$DATA_DIR/applications" "$DATA_DIR/icons/hicolor/256x256/apps"
    $SUDO install -m 755 "$bin" "$BIN_DIR/rustspace.new"
    $SUDO mv -f "$BIN_DIR/rustspace.new" "$BIN_DIR/rustspace"
    $SUDO install -m 644 "$png" "$DATA_DIR/icons/hicolor/256x256/apps/rustspace.png"
    sed "s|^Exec=.*|Exec=$BIN_DIR/rustspace|" "$desktop" > "${TMPDIR:-/tmp}/$APP_ID.desktop"
    $SUDO install -m 644 "${TMPDIR:-/tmp}/$APP_ID.desktop" "$DATA_DIR/applications/$APP_ID.desktop"
    rm -f "${TMPDIR:-/tmp}/$APP_ID.desktop"
    command -v update-desktop-database >/dev/null 2>&1 \
        && $SUDO update-desktop-database "$DATA_DIR/applications" >/dev/null 2>&1 || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 \
        && $SUDO gtk-update-icon-cache -q "$DATA_DIR/icons/hicolor" >/dev/null 2>&1 || true
    say "Готово: $("$BIN_DIR/rustspace" --version 2>/dev/null || echo rustspace)"
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *) say "Добавьте $BIN_DIR в PATH, чтобы запускать командой rustspace" ;;
    esac
    say "Rustspace появился в меню приложений. Обновить — запустить этот скрипт ещё раз."
}

HERE=$(CDPATH="" cd -- "$(dirname -- "$0")" 2>/dev/null && pwd || true)
if [ -n "$HERE" ] && [ -x "$HERE/rustspace" ] && [ -f "$HERE/$APP_ID.desktop" ]; then
    say "Ставлю готовую сборку из $HERE"
    install_files "$HERE/rustspace" "$HERE/rustspace.png" "$HERE/$APP_ID.desktop"
    exit 0
fi

# --- Системные пакеты для сборки -------------------------------------------
detect_pm() {
    for pm in pacman apt-get dnf zypper; do
        command -v "$pm" >/dev/null 2>&1 && { echo "$pm"; return; }
    done
    echo none
}

install_deps() {
    case "$1" in
        pacman) set -- sudo pacman -S --needed --noconfirm \
            base-devel git curl clang cmake pkgconf alsa-lib libpipewire gtk3 \
            libayatana-appindicator libxkbcommon wayland libx11 libxrandr libxcb mesa \
            libva libdrm libxext libxfixes libxdamage libxcomposite libxtst dbus glib2 ;;
        apt-get) set -- sudo sh -c 'apt-get update && apt-get install -y --no-install-recommends \
            build-essential git curl ca-certificates clang cmake pkg-config \
            libasound2-dev libdbus-1-dev libpipewire-0.3-dev libgbm-dev libxkbcommon-dev \
            libgtk-3-dev libayatana-appindicator3-dev libwayland-dev libx11-dev libxrandr-dev \
            libxcb1-dev libegl1-mesa-dev libglib2.0-dev libva-dev libdrm-dev libxext-dev \
            libxfixes-dev libxdamage-dev libxcomposite-dev libxtst-dev' ;;
        dnf) set -- sudo dnf install -y \
            gcc gcc-c++ git curl clang cmake pkgconf-pkg-config alsa-lib-devel dbus-devel \
            pipewire-devel mesa-libgbm-devel libxkbcommon-devel gtk3-devel \
            libayatana-appindicator-gtk3-devel wayland-devel libX11-devel libXrandr-devel \
            libxcb-devel mesa-libEGL-devel glib2-devel libva-devel libdrm-devel libXext-devel \
            libXfixes-devel libXdamage-devel libXcomposite-devel libXtst-devel ;;
        zypper) set -- sudo zypper --non-interactive install \
            gcc gcc-c++ git curl clang cmake pkg-config alsa-devel dbus-1-devel pipewire-devel \
            libgbm-devel libxkbcommon-devel gtk3-devel libayatana-appindicator3-devel \
            wayland-devel libX11-devel libXrandr-devel libxcb-devel Mesa-libEGL-devel \
            glib2-devel libva-devel libdrm-devel libXext-devel libXfixes-devel \
            libXdamage-devel libXcomposite-devel libXtst-devel ;;
        *) warn "не знаю менеджер пакетов этой системы: поставьте сами git, clang, cmake, pkg-config и заголовки ALSA, PipeWire, GTK 3, ayatana-appindicator, xkbcommon, Wayland/X11, libva, libdrm"
           return 0 ;;
    esac
    say "Нужные для сборки пакеты: $*"
    if ask "Поставить их (понадобится пароль sudo)?"; then
        "$@" || die "пакеты не поставились"
    else
        warn "пропускаю пакеты: если сборка упадёт, поставьте их сами"
    fi
}

if [ "$DEPS" = 1 ]; then
    install_deps "$(detect_pm)"
fi
command -v git >/dev/null 2>&1 || die "нужен git"

# --- Rust ---------------------------------------------------------------------
if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
    PATH="$HOME/.cargo/bin:$PATH"
fi
if ! command -v cargo >/dev/null 2>&1; then
    say "Rust (cargo) не найден. Его ставит rustup — официальный установщик Rust."
    ask "Поставить Rust через rustup в ~/.cargo?" || die "без Rust не собрать: https://rustup.rs"
    command -v curl >/dev/null 2>&1 || die "нужен curl"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
        || die "rustup не поставился"
    PATH="$HOME/.cargo/bin:$PATH"
fi

# --- Исходники -----------------------------------------------------------------
if [ -d "$SRC_DIR/.git" ]; then
    say "Обновляю исходники в $SRC_DIR"
    git -C "$SRC_DIR" fetch --tags --prune origin || die "не удалось обновить исходники"
else
    say "Скачиваю исходники в $SRC_DIR"
    mkdir -p "$(dirname "$SRC_DIR")"
    git clone "https://github.com/$REPO.git" "$SRC_DIR" || die "не удалось скачать исходники"
fi
if git -C "$SRC_DIR" rev-parse --verify --quiet "origin/$REF" >/dev/null; then
    git -C "$SRC_DIR" checkout --quiet --detach "origin/$REF"
else
    git -C "$SRC_DIR" checkout --quiet --detach "$REF" || die "нет ветки, тега или коммита $REF"
fi
say "Собираю $(git -C "$SRC_DIR" describe --tags --always 2>/dev/null || echo "$REF")"

# --- Голос: libwebrtc собирается clang 21+ ---------------------------------
FEATURES="desktop"
if [ "$VOICE" = 1 ]; then
    clang_major() { "$1" --version 2>/dev/null | sed -n 's/.*clang version \([0-9][0-9]*\).*/\1/p' | head -n 1; }
    best=""
    for cc in clang clang-23 clang-22 clang-21; do
        command -v "$cc" >/dev/null 2>&1 || continue
        major=$(clang_major "$cc")
        if [ -n "$major" ] && [ "$major" -ge 21 ]; then best="$cc"; break; fi
    done
    if [ -n "$best" ]; then
        FEATURES="voice,camera,desktop"
        CC="$best"
        CXX=$(echo "$best" | sed 's/clang/clang++/')
        export CC CXX
        libdir=$("$best" -print-resource-dir 2>/dev/null | sed 's|/lib/clang/.*|/lib|')
        [ -n "$libdir" ] && [ -d "$libdir" ] && export LIBCLANG_PATH="${LIBCLANG_PATH:-$libdir}"
        say "Голос и камера: собираю с $best"
    else
        warn "для голоса нужен clang 21 или новее (на Debian/Ubuntu — https://apt.llvm.org)."
        warn "Собираю без голоса и камеры; поставьте clang 21+ и запустите скрипт снова."
    fi
fi

cd "$SRC_DIR"
cargo build --release --locked -p bs-app --features "$FEATURES" \
    || die "сборка не удалась (см. ошибки выше)"

install_files "$SRC_DIR/target/release/rustspace" \
    "$SRC_DIR/packaging/linux/rustspace.png" \
    "$SRC_DIR/packaging/linux/$APP_ID.desktop"
