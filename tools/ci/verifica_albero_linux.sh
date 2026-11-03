#!/usr/bin/env bash
# Guardia: i fatti dichiarati in Cargo.toml sull'albero Linux del vault devono
# essere veri. Non prova il Secret Service: quella prova è il test del vault.
set -euo pipefail
cd "$(dirname "$0")/../.."

TARGET=x86_64-unknown-linux-gnu
ALBERO=(cargo tree --workspace --locked --all-features --target "$TARGET")

if "${ALBERO[@]}" -i zbus >/dev/null 2>&1; then
  echo "zbus è tornato nell'albero Linux: il vault deve usare dbus-secret-service" >&2
  exit 1
fi

if "${ALBERO[@]}" -i keyring >/dev/null 2>&1; then
  echo "il facade keyring è nell'albero Linux: la sua feature v1 riporta zbus" >&2
  exit 1
fi

for crate in dbus-secret-service-keyring-store dbus-secret-service libdbus-sys; do
  if ! "${ALBERO[@]}" -i "$crate" >/dev/null 2>&1; then
    echo "$crate non è nell'albero Linux: il vault non usa più lo store previsto" >&2
    exit 1
  fi
done

if ! "${ALBERO[@]}" -e features -i libdbus-sys | grep -q 'libdbus-sys feature "vendored"'; then
  echo "libdbus-sys non è vendored: servirebbe libdbus-1-dev e la libdbus del sistema" >&2
  exit 1
fi

echo "ok: su Linux il vault usa dbus-secret-service con libdbus vendored, senza zbus"
