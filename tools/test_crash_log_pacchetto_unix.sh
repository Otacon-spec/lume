#!/usr/bin/env bash
# Esegue il binario della release con il panic controllato e controlla il log.
set -euo pipefail

archivio="${1:?uso: test_crash_log_pacchetto_unix.sh <archivio.tar.gz>}"
base="$(basename "$archivio" .tar.gz)"
case "$base" in
    lume-[A-Za-z0-9._-]*) ;;
    *) echo "nome archivio non valido: $base" >&2; exit 2 ;;
esac

temporaneo="$(mktemp -d)"
trap 'rm -rf "$temporaneo"' EXIT
binario="$temporaneo/lume"
tar -xOzf "$archivio" "$base/lume" > "$binario"
chmod 755 "$binario"

set +e
LUME_DATA_DIR="$temporaneo/dati" "$binario" --prova-crash \
    > "$temporaneo/uscita.log" 2>&1
stato=$?
set -e
if [[ "$stato" -ne 101 ]]; then
    cat "$temporaneo/uscita.log" >&2
    echo "il crash controllato deve terminare con 101, ricevuto $stato" >&2
    exit 1
fi

shopt -s nullglob
log_crash=("$temporaneo/dati/logs"/crash-*.log)
if [[ "${#log_crash[@]}" -ne 1 ]]; then
    echo "atteso un solo crash log, trovati ${#log_crash[@]}" >&2
    exit 1
fi

python3 - "$temporaneo" "$(cd "$(dirname "$0")/.." && pwd)" <<'PY'
import hashlib
import os
import pathlib
import sys

temp = pathlib.Path(sys.argv[1])
repo = pathlib.Path(sys.argv[2])
logs = list((temp / "dati" / "logs").glob("crash-*.log"))
assert len(logs) == 1, logs
data = logs[0].read_bytes()
text = data.decode("utf-8")
assert "Messaggio: crash di prova richiesto con --prova-crash" in text
assert "Punto: crates/lume-app/src/main.rs:" in text or \
    "Punto: /lume/crates/lume-app/src/main.rs:" in text

home = os.environ.get("USERPROFILE") or os.environ.get("HOME", "")
markers = [
    home,
    os.environ.get("CARGO_HOME", str(pathlib.Path(home) / ".cargo")),
    os.environ.get("RUSTUP_HOME", str(pathlib.Path(home) / ".rustup")),
    str(repo),
    ".cargo",
    ".rustup",
]
folded_data = data.lower()
for encoding in ("utf-16le", "utf-16be"):
    for marker in markers:
        if marker:
            needle = marker.lower().encode(encoding)
            assert needle not in data, f"percorso personale nel crash log: {marker}"
for marker in filter(None, markers):
    assert marker.lower().encode("utf-8") not in folded_data, \
        f"percorso personale nel crash log: {marker}"

print(f"crash log release valido; percorsi personali assenti; sha256={hashlib.sha256(data).hexdigest()}")
PY
