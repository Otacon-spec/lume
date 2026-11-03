#!/usr/bin/env bash
# Verifica il verificatore con archivi finti, senza compilare il progetto.
set -euo pipefail

QUI="$(cd "$(dirname "$0")" && pwd)"
VERIFICA="$QUI/verifica_pacchetto_unix.sh"
LAVORO="$(mktemp -d)"
trap 'rm -rf "$LAVORO"' EXIT

COMMIT=0123456789abcdef0123456789abcdef01234567
SHA12=0123456789ab
NOME="lume-0.0.0-prova-$SHA12"

mkdir -p "$LAVORO/bin"
cat > "$LAVORO/bin/rustc" <<'SH'
#!/usr/bin/env bash
case "${1:-}" in
    -vV) printf 'host: x86_64-unknown-linux-gnu\n' ;;
    --version) printf 'rustc 1.96.0 (fake verificatore)\n' ;;
    *) exit 1 ;;
esac
SH
cat > "$LAVORO/bin/cargo" <<'SH'
#!/usr/bin/env bash
case "${1:-}" in
    --version) printf 'cargo 1.96.0 (fake verificatore)\n' ;;
    *) exit 1 ;;
esac
SH
chmod 755 "$LAVORO/bin/rustc" "$LAVORO/bin/cargo"
export PATH="$LAVORO/bin:$PATH"
# shellcheck source=tools/lib_release.sh
. "$QUI/lib_release.sh"

crea_albero() {
    local dir="$1"
    mkdir -p "$dir/licenze"
    printf '#!/bin/sh\nexit 0\n' > "$dir/lume"
    printf '#!/bin/sh\necho "invalid Lume process-watchdog configuration: prova" >&2\nexit 127\n' > "$dir/lume-process-watchdog"
    printf '#!/bin/sh\necho "Lume Git supervisor: missing supervisor data" >&2\nexit 127\n' > "$dir/lume-git-supervisor"
    chmod 755 "$dir/lume" "$dir/lume-process-watchdog" "$dir/lume-git-supervisor"
    for f in LICENSE.txt NOTICE.txt THIRD_PARTY_LICENSES.md THIRD_PARTY_NOTICES.md \
        licenze/Inter-OFL.txt licenze/JetBrainsMono-OFL.txt \
        licenze/material-icon-theme-MIT.txt licenze/catppuccin-vscode-icons-MIT.txt \
        licenze/tiberius-ng-LICENSE-MIT.txt licenze/tiberius-ng-LICENSE-APACHE.txt \
        licenze/aws-lc-sys-LICENSE.txt; do
        printf 'testo di prova\n' > "$dir/$f"
    done
}

impacchetta() {
    local dir="$1" archivio="$2"
    release_finalizza_tar "$dir" "$archivio" 0.0.0 "$COMMIT" \
        x86_64-unknown-linux-gnu prova default >/dev/null
}

a_tree() {
    local cartella="$1"
    local dir="$LAVORO/$cartella/$NOME"
    crea_albero "$dir"
    impacchetta "$dir" "$dir.tar.gz"
    printf '%s\n' "$dir"
}

pulisci_output() {
    local dir="$1"
    rm -f "$dir/MANIFEST.json" "$dir.tar.gz" "$dir.tar.gz.sha256" \
        "$dir.tar.gz.MANIFEST.json"
}

fallimenti=0
atteso_ok() {
    local nome="$1" archivio="$2"
    if bash "$VERIFICA" "$archivio" "$COMMIT" >"$LAVORO/uscita.txt" 2>&1; then
        echo "ok: $nome"
    else
        echo "FALLITO: $nome doveva passare"
        cat "$LAVORO/uscita.txt"
        fallimenti=$((fallimenti + 1))
    fi
}

atteso_rifiuto() {
    local nome="$1" archivio="$2" messaggio="$3"
    if bash "$VERIFICA" "$archivio" "$COMMIT" >"$LAVORO/uscita.txt" 2>&1; then
        echo "FALLITO: $nome è stato accettato"
        fallimenti=$((fallimenti + 1))
    elif grep -q -- "$messaggio" "$LAVORO/uscita.txt"; then
        echo "ok: $nome respinto con «$messaggio»"
    else
        echo "FALLITO: $nome respinto, ma senza «$messaggio»"
        cat "$LAVORO/uscita.txt"
        fallimenti=$((fallimenti + 1))
    fi
}

A="$(a_tree valido)"
atteso_ok "pacchetto completo" "$A.tar.gz"

B="$(a_tree senza-watchdog)"
rm "$B/lume-process-watchdog"
pulisci_output "$B"
impacchetta "$B" "$B.tar.gz"
atteso_rifiuto "senza watchdog" "$B.tar.gz" "manca: lume-process-watchdog"

BG="$(a_tree senza-git-supervisor)"
rm "$BG/lume-git-supervisor"
pulisci_output "$BG"
impacchetta "$BG" "$BG.tar.gz"
atteso_rifiuto "senza supervisor Git" "$BG.tar.gz" "manca: lume-git-supervisor"

C="$(a_tree senza-lume)"
rm "$C/lume"
pulisci_output "$C"
impacchetta "$C" "$C.tar.gz"
atteso_rifiuto "senza lume" "$C.tar.gz" "manca: lume"

D="$(a_tree non-eseguibile)"
chmod 644 "$D/lume-process-watchdog"
pulisci_output "$D"
impacchetta "$D" "$D.tar.gz"
atteso_rifiuto "watchdog non eseguibile" "$D.tar.gz" "non eseguibile: lume-process-watchdog"

E="$(a_tree finto)"
printf '#!/bin/sh\nexit 0\n' > "$E/lume-process-watchdog"
chmod 755 "$E/lume-process-watchdog"
pulisci_output "$E"
impacchetta "$E" "$E.tar.gz"
atteso_rifiuto "watchdog che non è il nostro" "$E.tar.gz" "il watchdog non risponde come atteso"

F="$(a_tree senza-licenza)"
rm "$F/licenze/aws-lc-sys-LICENSE.txt"
pulisci_output "$F"
impacchetta "$F" "$F.tar.gz"
atteso_rifiuto "senza aws-lc-sys-LICENSE.txt" "$F.tar.gz" "manca: licenze/aws-lc-sys-LICENSE.txt"

# Contenuto alterato dopo la scrittura del manifest: gli hash non tornano.
mkdir "$LAVORO/manomesso"
tar -C "$LAVORO/manomesso" -xzf "$A.tar.gz"
printf 'alterato\n' >> "$LAVORO/manomesso/$NOME/lume"
tar -C "$LAVORO/manomesso" -czf "$LAVORO/manomesso/$NOME.tar.gz" "$NOME"
cp "$A.tar.gz.MANIFEST.json" "$LAVORO/manomesso/$NOME.tar.gz.MANIFEST.json"
release_sha256_nome "$LAVORO/manomesso/$NOME.tar.gz" \
    > "$LAVORO/manomesso/$NOME.tar.gz.sha256"
atteso_rifiuto "contenuto alterato" "$LAVORO/manomesso/$NOME.tar.gz" "sha256/size diversi"

# Manifest assente nell'archivio.
mkdir -p "$LAVORO/senza-manifest/$NOME"
crea_albero "$LAVORO/senza-manifest/$NOME"
tar -C "$LAVORO/senza-manifest" -czf "$LAVORO/senza-manifest/$NOME.tar.gz" "$NOME"
release_sha256_nome "$LAVORO/senza-manifest/$NOME.tar.gz" \
    > "$LAVORO/senza-manifest/$NOME.tar.gz.sha256"
atteso_rifiuto "senza manifest" "$LAVORO/senza-manifest/$NOME.tar.gz" "MANIFEST.json"

# Copia del manifest accanto all'artefatto assente.
mkdir "$LAVORO/senza-copia"
cp "$A.tar.gz" "$LAVORO/senza-copia/$NOME.tar.gz"
cp "$A.tar.gz.sha256" "$LAVORO/senza-copia/$NOME.tar.gz.sha256"
atteso_rifiuto "senza copia del manifest" "$LAVORO/senza-copia/$NOME.tar.gz" \
    "manca la copia del manifest"

# Nome senza sha12.
mkdir "$LAVORO/senza-sha"
cp "$A.tar.gz" "$LAVORO/senza-sha/lume-0.0.0-prova.tar.gz"
cp "$A.tar.gz.MANIFEST.json" "$LAVORO/senza-sha/lume-0.0.0-prova.tar.gz.MANIFEST.json"
release_sha256_nome "$LAVORO/senza-sha/lume-0.0.0-prova.tar.gz" \
    > "$LAVORO/senza-sha/lume-0.0.0-prova.tar.gz.sha256"
atteso_rifiuto "nome senza sha12" "$LAVORO/senza-sha/lume-0.0.0-prova.tar.gz" \
    "nome artefatto inatteso"

# Commit diverso da quello atteso.
if bash "$VERIFICA" "$A.tar.gz" 0000000000000000000000000000000000000000 \
    >"$LAVORO/uscita.txt" 2>&1; then
    echo "FALLITO: commit diverso accettato"
    fallimenti=$((fallimenti + 1))
elif grep -q 'diverso da quello atteso' "$LAVORO/uscita.txt"; then
    echo "ok: commit diverso respinto"
else
    echo "FALLITO: commit diverso respinto senza messaggio"
    cat "$LAVORO/uscita.txt"
    fallimenti=$((fallimenti + 1))
fi

# Nessun archive valido senza manifest: il vecchio formato non passa più.
cp "$A.tar.gz" "$LAVORO/manomesso.tar.gz"
printf '%064d\n' 0 > "$LAVORO/manomesso.tar.gz.sha256"
atteso_rifiuto "somma sbagliata" "$LAVORO/manomesso.tar.gz" "la somma di controllo non corrisponde"

if [ "$fallimenti" -ne 0 ]; then
    echo "$fallimenti verifiche non passate" >&2
    exit 1
fi
echo "tutte le verifiche del verificatore sono passate"
