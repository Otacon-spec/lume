#!/usr/bin/env bash
# Libreria condivisa dagli ingressi di release Lume.
#
# Fattorizza le parti testabili senza compilare: commit e albero pulito,
# nome dell'artefatto con lo SHA, manifest dei file e rifiuto degli output
# già esistenti. Gli script di produzione (pacchetto_unix.sh) e i test
# sorgono questo file, così usano esattamente le stesse funzioni.
#
# Non esegue nulla al source. Compatibile con bash 3.2 (macOS).

_RELEASE_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

release_errore() {
    printf '%s\n' "$*" >&2
}

release_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

release_sha256_nome() {
    printf '%s  %s\n' "$(release_sha256 "$1")" "$(basename "$1")"
}

release_valida_sha() {
    case "${1:-}" in
        *[!0-9a-f]*|'')
            release_errore "commit non valido: ${1:-vuoto} (attesi 40 caratteri esadecimali)"
            return 1
            ;;
    esac
    [ "${#1}" -eq 40 ] || {
        release_errore "commit non valido: $1 (attesi 40 caratteri esadecimali)"
        return 1
    }
}

# Legge HEAD dal repository indicato (default: directory corrente).
release_commit() {
    local radice="${1:-.}" sha
    if ! sha="$(git -C "$radice" rev-parse --verify HEAD 2>/dev/null)"; then
        release_errore "impossibile leggere HEAD dal repository Git: $radice"
        return 1
    fi
    sha="$(printf '%s' "$sha" | tr 'A-F' 'a-f')"
    release_valida_sha "$sha" || return 1
    printf '%s\n' "$sha"
}

# Rifiuta modifiche tracked/staged/untracked: una release nasce da un commit.
release_require_clean_tree() {
    local radice="${1:-.}" stato
    if ! stato="$(git -C "$radice" status --porcelain --untracked-files=normal)"; then
        release_errore "impossibile verificare lo stato Git"
        return 1
    fi
    if [ -n "$stato" ]; then
        release_errore "release rifiutata: albero Git non pulito"
        printf '%s\n' "$stato" >&2
        return 1
    fi
}

release_version() {
    local radice="${1:-.}" versione
    versione="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$radice/Cargo.toml" | head -n 1)"
    if [ -z "$versione" ]; then
        release_errore "versione non trovata in $radice/Cargo.toml"
        return 1
    fi
    case "$versione" in
        *[!A-Za-z0-9.+_-]*)
            release_errore "versione non valida in $radice/Cargo.toml: $versione"
            return 1
            ;;
    esac
    printf '%s\n' "$versione"
}

release_valida_etichetta() {
    case "${1:-}" in
        *[!A-Za-z0-9._-]*|'')
            release_errore "etichetta non valida: ${1:-vuota} (lettere, numeri, punto, trattino o underscore)"
            return 1
            ;;
    esac
}

# Nome base dell'artefatto: lume-<versione>-<target>-<sha12>.
release_artifact_basename() {
    local versione="$1" etichetta="$2" sha12="$3"
    release_valida_etichetta "$etichetta" || return 1
    case "$versione" in
        *[!A-Za-z0-9.+_-]*|'')
            release_errore "versione non valida per il nome: ${versione:-vuota}"
            return 1
            ;;
    esac
    case "$sha12" in
        *[!0-9a-f]*|'')
            release_errore "sha12 non valido: ${sha12:-vuoto}"
            return 1
            ;;
    esac
    [ "${#sha12}" -eq 12 ] || {
        release_errore "sha12 non valido: $sha12 (attesi 12 caratteri)"
        return 1
    }
    printf 'lume-%s-%s-%s\n' "$versione" "$etichetta" "$sha12"
}

# Triple del target compilato: la host di rustc (build native), non l'etichetta.
release_target_triple() {
    local versione triple
    # Prima tutto l'output, poi il filtro: con pipefail, un awk che esce alla
    # prima riga utile fa morire rustc di SIGPIPE e la pipeline fallisce a caso.
    if ! versione="$(rustc -vV 2>/dev/null)"; then
        release_errore "impossibile leggere la triple da rustc -vV"
        return 1
    fi
    triple="$(printf '%s\n' "$versione" | awk '/^host: /{print $2}')"
    if [ -z "$triple" ]; then
        release_errore "rustc -vV non riporta la triple host"
        return 1
    fi
    printf '%s\n' "$triple"
}

release_rustc_version() {
    local versione
    if ! versione="$(rustc --version 2>/dev/null)"; then
        release_errore "impossibile leggere rustc --version"
        return 1
    fi
    [ -n "$versione" ] || {
        release_errore "rustc --version non ha prodotto output"
        return 1
    }
    printf '%s\n' "$versione"
}

release_cargo_version() {
    local versione
    if ! versione="$(cargo --version 2>/dev/null)"; then
        release_errore "impossibile leggere cargo --version"
        return 1
    fi
    [ -n "$versione" ] || {
        release_errore "cargo --version non ha prodotto output"
        return 1
    }
    printf '%s\n' "$versione"
}

# Selezione delle feature usata dalla build: la default, salvo override esplicito.
release_cargo_features() {
    printf '%s\n' "${LUME_RELEASE_FEATURES:-default}"
}

release_data_utc() {
    date -u +%Y-%m-%dT%H:%M:%SZ
}

# Rifiuta qualunque output già presente, senza rimuoverlo.
release_rifiuta_esistenti() {
    local percorso esistente=0
    for percorso in "$@"; do
        if [ -e "$percorso" ] || [ -L "$percorso" ]; then
            release_errore "release rifiutata: output già esistente: $percorso"
            esistente=1
        fi
    done
    [ "$esistente" -eq 0 ]
}

# Scrive MANIFEST.json nella radice dello stage. Il file non deve esistere.
release_scrivi_manifest() {
    local radice="$1" manifest="$2" versione="$3" commit="$4"
    local target="$5" etichetta="$6" features="$7"
    local rustc_versione cargo_versione data_utc
    release_valida_sha "$commit" || return 1
    release_rifiuta_esistenti "$manifest" || return 1
    data_utc="$(release_data_utc)"
    rustc_versione="$(release_rustc_version)" || return 1
    cargo_versione="$(release_cargo_version)" || return 1
    python3 - "$radice" "$manifest" "$versione" "$commit" "$target" "$etichetta" \
        "$features" "$data_utc" "$rustc_versione" "$cargo_versione" <<'PY'
import hashlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
manifest = pathlib.Path(sys.argv[2])
version, commit, target, label, features_raw, build_date, rustc_v, cargo_v = sys.argv[3:11]

features = [part.strip() for part in features_raw.split(",") if part.strip()]
if not features:
    raise SystemExit("feature cargo non valide: elenco vuoto")

files = {}
for path in sorted(root.rglob("*")):
    if path.is_symlink():
        raise SystemExit(f"collegamento simbolico non ammesso nello stage: {path}")
    if not path.is_file():
        continue
    if path == root / "MANIFEST.json":
        continue
    rel = path.relative_to(root).as_posix()
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
            size += len(block)
    files[rel] = {
        "sha256": digest.hexdigest(),
        "size": size,
    }

documento = {
    "schema": 1,
    "package": "lume",
    "version": version,
    "commit": commit,
    "build_date_utc": build_date,
    "rustc": rustc_v,
    "cargo": cargo_v,
    "target_triple": target,
    "target_label": label,
    "cargo_features": features,
    "files": files,
}

manifest.parent.mkdir(parents=True, exist_ok=True)
with manifest.open("x", encoding="utf-8") as handle:
    json.dump(documento, handle, indent=2, sort_keys=True)
    handle.write("\n")
PY
}

# Crea archivio, copia MANIFEST.json accanto e scrive il .sha256.
# Lo stage deve esistere già, con dentro i file definitivi.
release_finalizza_tar() {
    local stage="$1" archivio="$2" versione="$3" commit="$4"
    local target="$5" etichetta="$6" features="$7"
    local base parent manifest
    if [ ! -d "$stage" ]; then
        release_errore "stage inesistente: $stage"
        return 1
    fi
    case "$archivio" in
        /*) ;;
        *) archivio="$(pwd)/$archivio" ;;
    esac
    base="$(basename "$stage")"
    parent="$(cd "$(dirname "$stage")" && pwd)"
    manifest="$parent/$base/MANIFEST.json"
    [ "$(basename "$archivio")" = "$base.tar.gz" ] || {
        release_errore "nome archivio incoerente: $(basename "$archivio"), atteso $base.tar.gz"
        return 1
    }
    if [ "$(dirname "$archivio")" != "$parent" ]; then
        release_errore "archivio e stage devono stare nella stessa directory"
        return 1
    fi
    release_rifiuta_esistenti "$manifest" "$archivio" "$archivio.sha256" \
        "$archivio.MANIFEST.json" || return 1
    release_scrivi_manifest "$parent/$base" "$manifest" "$versione" "$commit" \
        "$target" "$etichetta" "$features" || return 1
    if ! tar -C "$parent" -czf "$archivio" "$base"; then
        release_errore "creazione dell'archivio fallita: $archivio"
        return 1
    fi
    cp "$manifest" "$archivio.MANIFEST.json"
    release_sha256_nome "$archivio" > "$archivio.sha256"
    printf '%s\n' "$archivio"
}

# Verifica manifest, contenuti e commit dichiarato di un artefatto (tar.gz o zip).
release_verifica_manifest() {
    local artefatto="$1" commit="$2"
    python3 "$_RELEASE_LIB_DIR/verifica_manifest_artefatto.py" "$artefatto" "$commit"
}
