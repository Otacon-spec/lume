#!/usr/bin/env bash
# Verifica il contenuto, il manifest e il watchdog di un pacchetto Unix di Lume.
# Uso: bash tools/verifica_pacchetto_unix.sh <archivio.tar.gz> [commit]
#
# Senza il secondo argomento il commit atteso è HEAD del repository: il
# manifest deve dichiarare quello SHA e il nome dell'archivio il suo sha12.
set -euo pipefail

sorgente="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=tools/lib_release.sh
. "$sorgente/lib_release.sh"

archivio="${1:?uso: verifica_pacchetto_unix.sh <archivio.tar.gz> [commit]}"
[ -f "$archivio" ] || { echo "archivio inesistente: $archivio" >&2; exit 1; }

if [ -n "${2:-}" ]; then
    commit="$2"
    release_valida_sha "$commit" || exit 1
else
    radice_repo="$(cd "$sorgente/.." && pwd)"
    commit="$(release_commit "$radice_repo")" || {
        echo "specificare il commit atteso oppure eseguire dentro il repository Git" >&2
        exit 1
    }
fi

if [ -f "$archivio.sha256" ]; then
    attesa="$(awk '{print $1}' "$archivio.sha256")"
    trovata="$(release_sha256 "$archivio")"
    if [ "$attesa" != "$trovata" ]; then
        echo "la somma di controllo non corrisponde: attesa $attesa, trovata $trovata" >&2
        exit 1
    fi
fi

release_verifica_manifest "$archivio" "$commit" || exit 1

lavoro="$(mktemp -d)"
trap 'rm -rf "$lavoro"' EXIT
tar -C "$lavoro" -xzf "$archivio"
radice="$(find "$lavoro" -mindepth 1 -maxdepth 1 -type d -print | head -n 1)"
[ -n "$radice" ] || { echo "l'archivio non contiene una cartella" >&2; exit 1; }

mancanti=0
for f in lume lume-process-watchdog lume-git-supervisor LICENSE.txt NOTICE.txt THIRD_PARTY_LICENSES.md \
    THIRD_PARTY_NOTICES.md licenze/Inter-OFL.txt licenze/JetBrainsMono-OFL.txt \
    licenze/material-icon-theme-MIT.txt licenze/catppuccin-vscode-icons-MIT.txt \
    licenze/tiberius-ng-LICENSE-MIT.txt licenze/tiberius-ng-LICENSE-APACHE.txt \
    licenze/aws-lc-sys-LICENSE.txt; do
    if [ ! -f "$radice/$f" ]; then
        echo "manca: $f" >&2
        mancanti=$((mancanti + 1))
    fi
done
[ "$mancanti" -eq 0 ] || exit 1

for f in lume lume-process-watchdog lume-git-supervisor; do
    [ -x "$radice/$f" ] || { echo "non eseguibile: $f" >&2; exit 1; }
done

set +e
uscita="$(env -i "$radice/lume-process-watchdog" 2>&1)"
codice=$?
set -e
if [ "$codice" -ne 127 ] || ! printf '%s' "$uscita" | grep -q "invalid Lume process-watchdog configuration"; then
    echo "il watchdog non risponde come atteso (codice $codice): $uscita" >&2
    exit 1
fi

set +e
uscita="$(env -i "$radice/lume-git-supervisor" 2>&1)"
codice=$?
set -e
if [ "$codice" -ne 127 ] || ! printf '%s' "$uscita" | grep -q "Lume Git supervisor: missing supervisor data"; then
    echo "il supervisor Git non risponde come atteso (codice $codice): $uscita" >&2
    exit 1
fi

echo "pacchetto valido: $archivio"
