#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "uso: $0 /percorso/al/binario-lume" >&2
    exit 2
fi

for comando in xvfb-run xwininfo realpath mktemp; do
    if ! command -v "$comando" >/dev/null 2>&1; then
        echo "manca il comando richiesto: $comando" >&2
        exit 2
    fi
done

binario=$(realpath "$1")
if [[ ! -x "$binario" ]]; then
    echo "binario Lume assente o non eseguibile: $binario" >&2
    exit 2
fi

directory_dati=$(mktemp -d "${TMPDIR:-/tmp}/lume-xvfb-dati.XXXXXX")
chmod 700 "$directory_dati"
log=$(mktemp "${TMPDIR:-/tmp}/lume-xvfb.XXXXXX.log")

xvfb-run -a -s "-screen 0 1600x1000x24" bash -c '
    set -euo pipefail
    binario=$1
    directory_dati=$2
    log=$3

    env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET \
        LUME_DATA_DIR="$directory_dati" "$binario" >"$log" 2>&1 &
    app_pid=$!
    termina_app() {
        kill -TERM "$app_pid" 2>/dev/null || true
        wait "$app_pid" 2>/dev/null || true
    }
    trap termina_app EXIT

    for _ in $(seq 1 30); do
        if ! kill -0 "$app_pid" 2>/dev/null; then
            echo "Lume è uscito prima di creare la finestra:" >&2
            cat "$log" >&2
            exit 1
        fi
        finestre=$(xwininfo -root -tree 2>/dev/null || true)
        if grep -Fq "\"Lume\"" <<<"$finestre"; then
            echo "finestra Lume rilevata sotto Xvfb"
            exit 0
        fi
        sleep 1
    done

    echo "Lume è rimasto attivo ma non ha creato la finestra entro 30 s:" >&2
    printf "%s\n" "$finestre" >&2
    cat "$log" >&2
    exit 1
' bash "$binario" "$directory_dati" "$log"
