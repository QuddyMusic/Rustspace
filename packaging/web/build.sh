#!/bin/sh
# Собирает веб-клиент Rustspace (egui → WebAssembly) в каталог.
#
#   packaging/web/build.sh [КАТАЛОГ]      по умолчанию target/web
#
# Нужны цель wasm32-unknown-unknown (rustup target add wasm32-unknown-unknown)
# и wasm-bindgen-cli той же версии, что wasm-bindgen в Cargo.lock: если его
# нет, ставится через cargo install. wasm-opt (binaryen), если есть, ужимает
# модуль. Рядом с большими файлами кладутся .gz — сервер отдаёт их браузерам.
set -eu

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
OUT=${1:-$ROOT/target/web}
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}
cd "$ROOT"

say() { printf '==> %s\n' "$*"; }

say "Собираю модуль клиента"
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
cargo build --release --locked -p bs-app --target wasm32-unknown-unknown

BINDGEN=$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"$/\1/p;}' Cargo.lock)
[ -n "$BINDGEN" ] || { echo "в Cargo.lock нет wasm-bindgen" >&2; exit 1; }
if ! wasm-bindgen --version 2>/dev/null | grep -q " $BINDGEN\$"; then
    say "Ставлю wasm-bindgen-cli $BINDGEN"
    cargo install --locked wasm-bindgen-cli --version "$BINDGEN"
fi

rm -rf "$OUT"
mkdir -p "$OUT"
wasm-bindgen --target web --no-typescript --out-dir "$OUT" --out-name rustspace \
    "$TARGET_DIR/wasm32-unknown-unknown/release/rustspace.wasm"
if command -v wasm-opt >/dev/null 2>&1; then
    say "Ужимаю модуль (wasm-opt)"
    wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int \
        -o "$OUT/rustspace_bg.opt.wasm" "$OUT/rustspace_bg.wasm"
    mv "$OUT/rustspace_bg.opt.wasm" "$OUT/rustspace_bg.wasm"
fi
for file in index.html rustspace.css start.js manifest.webmanifest icon-512.png; do
    cp "$ROOT/packaging/web/$file" "$OUT/"
done
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
echo "$VERSION" > "$OUT/.rustspace-web"
for file in rustspace_bg.wasm rustspace.js rustspace.css; do
    gzip -9 -k -n "$OUT/$file"
done
say "Готово: $OUT ($(du -sh "$OUT" | cut -f1))"
