#!/bin/sh
# Rustspace для Linux: установка в домашнюю папку, без root.
#
#   curl -fsSL https://raw.githubusercontent.com/QuddyMusic/Rustspace/main/packaging/linux/install.sh | sh
#
# Скачивает последний релиз под вашу архитектуру, сверяет контрольную сумму
# и кладёт:
#   ~/.local/bin/rustspace                                    — программа
#   ~/.local/share/applications/io.github.quddymusic.Rustspace.desktop
#   ~/.local/share/icons/hicolor/256x256/apps/rustspace.png
# Файл программы ваш, поэтому клиент потом обновляется сам (кнопка «Обновить»).
#
# Запуск из распакованного архива релиза ставит файлы из него, без скачивания.
# Параметры: --version 0.3.0 — конкретная версия; --uninstall — удалить.
set -eu

REPO="${RUSTSPACE_REPO:-QuddyMusic/Rustspace}"
APP_ID="io.github.quddymusic.Rustspace"
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
VERSION=""
UNINSTALL=0

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
die() { printf '\033[31mОшибка:\033[0m %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION="${2:-}"; shift 2 ;;
        --uninstall) UNINSTALL=1; shift ;;
        -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
        *) die "неизвестный параметр: $1" ;;
    esac
done

if [ "$UNINSTALL" = 1 ]; then
    rm -f "$BIN_DIR/rustspace" "$DATA_DIR/applications/$APP_ID.desktop" \
        "$DATA_DIR/icons/hicolor/256x256/apps/rustspace.png"
    say "Rustspace удалён. Настройки остались в ~/.config/rustspace"
    exit 0
fi

[ "$(id -u)" = 0 ] && die "запускайте без sudo: установка идёт в домашнюю папку"
case "$(uname -m)" in
    x86_64|amd64) ARCH=x86_64 ;;
    aarch64|arm64) ARCH=aarch64 ;;
    *) die "архитектура $(uname -m) не поддерживается" ;;
esac

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" 2>/dev/null && pwd || true)
if [ -n "$HERE" ] && [ -x "$HERE/rustspace" ]; then
    SRC="$HERE"
    say "Ставлю из $SRC"
else
    command -v curl >/dev/null 2>&1 || die "нужен curl"
    TMP=$(mktemp -d)
    trap 'rm -rf "$TMP"' EXIT
    NAME="rustspace-linux-$ARCH"
    if [ -n "$VERSION" ]; then
        BASE="https://github.com/$REPO/releases/download/v${VERSION#v}"
    else
        BASE="https://github.com/$REPO/releases/latest/download"
    fi
    say "Скачиваю $NAME.tar.gz"
    curl -fL --progress-bar -o "$TMP/$NAME.tar.gz" "$BASE/$NAME.tar.gz" \
        || die "не удалось скачать $BASE/$NAME.tar.gz"
    curl -fsSL -o "$TMP/$NAME.tar.gz.sha256" "$BASE/$NAME.tar.gz.sha256" \
        || die "нет контрольной суммы"
    (cd "$TMP" && sha256sum -c "$NAME.tar.gz.sha256" >/dev/null) \
        || die "контрольная сумма не совпала"
    tar xzf "$TMP/$NAME.tar.gz" -C "$TMP"
    SRC="$TMP/$NAME"
fi

mkdir -p "$BIN_DIR" "$DATA_DIR/applications" "$DATA_DIR/icons/hicolor/256x256/apps"
install -m 755 "$SRC/rustspace" "$BIN_DIR/rustspace.new"
mv -f "$BIN_DIR/rustspace.new" "$BIN_DIR/rustspace"
install -m 644 "$SRC/rustspace.png" "$DATA_DIR/icons/hicolor/256x256/apps/rustspace.png"
sed "s|^Exec=.*|Exec=$BIN_DIR/rustspace|" "$SRC/$APP_ID.desktop" \
    > "$DATA_DIR/applications/$APP_ID.desktop"
command -v update-desktop-database >/dev/null 2>&1 \
    && update-desktop-database "$DATA_DIR/applications" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 \
    && gtk-update-icon-cache -q "$DATA_DIR/icons/hicolor" >/dev/null 2>&1 || true

say "Готово: $("$BIN_DIR/rustspace" --version 2>/dev/null || echo rustspace)"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) say "Добавьте $BIN_DIR в PATH, чтобы запускать командой rustspace" ;;
esac
say "Rustspace появился в меню приложений. Удалить: sh install.sh --uninstall"
say "Голосу нужны системные библиотеки: на Arch — pipewire, alsa-lib, libpulse;"
say "на Debian/Ubuntu — libasound2, libpipewire-0.3-0, libgtk-3-0, libayatana-appindicator3-1"
