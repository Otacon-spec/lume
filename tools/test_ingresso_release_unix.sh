#!/usr/bin/env bash
# Verifica la barriera d'ingresso release con un repository Git reale, senza
# compilare: albero pulito, nome con lo SHA, manifest e rifiuto dell'overwrite.
set -euo pipefail

sorgente="$(cd "$(dirname "$0")" && pwd)"
lavoro="$(mktemp -d)"
trap 'rm -rf "$lavoro"' EXIT

repo="$lavoro/repo"
mkdir -p "$repo/tools" "$lavoro/bin" "$lavoro/artifacts"
cp "$sorgente/pacchetto_unix.sh" "$sorgente/lib_release.sh" \
    "$sorgente/verifica_pacchetto_unix.sh" \
    "$sorgente/verifica_manifest_artefatto.py" "$repo/tools/"

# Strumenti finti: questi test non compilano, ma il manifest deve comunque
# dichiarare versioni e triple non vuote.
cat > "$lavoro/bin/rustc" <<'SH'
#!/usr/bin/env bash
case "${1:-}" in
    -vV) printf 'host: x86_64-unknown-linux-gnu\n' ;;
    --version) printf 'rustc 1.96.0 (fake release ingressi)\n' ;;
    *) printf 'argomento rustc inatteso: %s\n' "${1:-}" >&2; exit 1 ;;
esac
SH
cat > "$lavoro/bin/cargo" <<'SH'
#!/usr/bin/env bash
case "${1:-}" in
    --version) printf 'cargo 1.96.0 (fake release ingressi)\n' ;;
    *) printf 'argomento cargo inatteso: %s\n' "${1:-}" >&2; exit 1 ;;
esac
SH
chmod 755 "$lavoro/bin/rustc" "$lavoro/bin/cargo"
export PATH="$lavoro/bin:$PATH"

git -C "$repo" init -q
git -C "$repo" config user.name Test
git -C "$repo" config user.email test@example.invalid
git -C "$repo" add tools/pacchetto_unix.sh tools/lib_release.sh \
    tools/verifica_pacchetto_unix.sh tools/verifica_manifest_artefatto.py
git -C "$repo" commit -qm iniziale

fallisci() {
    printf 'FALLITO: %s\n' "$*" >&2
    exit 1
}

# 1. Albero sporco: tracked, poi untracked.
printf 'modifica\n' >> "$repo/tools/pacchetto_unix.sh"
if bash "$repo/tools/pacchetto_unix.sh" linux-x86_64 >"$lavoro/log" 2>&1; then
    fallisci "albero con modifica tracked accettato"
fi
grep -q 'albero Git non pulito' "$lavoro/log" || fallisci "messaggio tracked assente"
git -C "$repo" add tools/pacchetto_unix.sh
git -C "$repo" commit -qm modifica

printf 'nuovo\n' > "$repo/nuovo"
if bash "$repo/tools/pacchetto_unix.sh" linux-x86_64 >"$lavoro/log" 2>&1; then
    fallisci "albero con untracked accettato"
fi
grep -q 'albero Git non pulito' "$lavoro/log" || fallisci "messaggio untracked assente"
rm "$repo/nuovo"

# 2. Nome, manifest e verifica sul commit reale del repository.
# shellcheck source=tools/lib_release.sh
. "$repo/tools/lib_release.sh"
commit="$(release_commit "$repo")"
release_valida_sha "$commit" || fallisci "helper non legge HEAD"
sha12="$(printf '%s' "$commit" | cut -c1-12)"
atteso="lume-0.3.0-linux-x86_64-$sha12"
nome="$(release_artifact_basename 0.3.0 linux-x86_64 "$sha12")"
[ "$nome" = "$atteso" ] || fallisci "nome inatteso: $nome"

stage="$lavoro/artifacts/$nome"
mkdir -p "$stage/licenze"
printf '#!/bin/sh\nexit 0\n' > "$stage/lume"
printf '#!/bin/sh\necho "invalid Lume process-watchdog configuration: prova" >&2\nexit 127\n' > "$stage/lume-process-watchdog"
printf '#!/bin/sh\necho "Lume Git supervisor: missing supervisor data" >&2\nexit 127\n' > "$stage/lume-git-supervisor"
chmod 755 "$stage/lume" "$stage/lume-process-watchdog" "$stage/lume-git-supervisor"
for f in LICENSE.txt NOTICE.txt THIRD_PARTY_LICENSES.md THIRD_PARTY_NOTICES.md \
    licenze/Inter-OFL.txt licenze/JetBrainsMono-OFL.txt \
    licenze/material-icon-theme-MIT.txt licenze/catppuccin-vscode-icons-MIT.txt \
    licenze/tiberius-ng-LICENSE-MIT.txt licenze/tiberius-ng-LICENSE-APACHE.txt \
    licenze/aws-lc-sys-LICENSE.txt; do
    printf 'testo di prova\n' > "$stage/$f"
done
archivio="$stage.tar.gz"
release_finalizza_tar "$stage" "$archivio" 0.3.0 "$commit" \
    x86_64-unknown-linux-gnu linux-x86_64 default >/dev/null

for f in "$archivio" "$archivio.sha256" "$archivio.MANIFEST.json"; do
    [ -f "$f" ] || fallisci "output mancante: $f"
done
case "$(basename "$archivio")" in
    "lume-0.3.0-linux-x86_64-$sha12.tar.gz") ;;
    *) fallisci "l'artefatto non porta lo sha12: $(basename "$archivio")" ;;
esac

release_verifica_manifest "$archivio" "$commit" >"$lavoro/verifica.log" 2>&1 ||
    { cat "$lavoro/verifica.log" >&2; fallisci "manifest valido rifiutato"; }
# Il verificatore di produzione risolve il commit da HEAD quando manca l'argomento.
bash "$repo/tools/verifica_pacchetto_unix.sh" "$archivio" \
    >"$lavoro/verifica-head.log" 2>&1 ||
    { cat "$lavoro/verifica-head.log" >&2; fallisci "verifica da HEAD fallita"; }
grep -q 'pacchetto valido' "$lavoro/verifica-head.log" ||
    fallisci "la verifica da HEAD non ha confermato il pacchetto"

python3 - "$archivio" "$commit" "$sha12" "$nome" <<'PY'
import hashlib
import json
import pathlib
import re
import sys
import tarfile

archivio = pathlib.Path(sys.argv[1])
commit, sha12, nome = sys.argv[2:5]
with tarfile.open(archivio, "r:gz") as archivio_tar:
    raw = archivio_tar.extractfile(f"{nome}/MANIFEST.json").read()
manifesto = json.loads(raw.decode("utf-8"))
assert manifesto["schema"] == 1, manifesto
assert manifesto["package"] == "lume", manifesto
assert manifesto["version"] == "0.3.0", manifesto
assert manifesto["commit"] == commit, manifesto
assert re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", manifesto["build_date_utc"]), manifesto
assert manifesto["rustc"] == "rustc 1.96.0 (fake release ingressi)", manifesto
assert manifesto["cargo"] == "cargo 1.96.0 (fake release ingressi)", manifesto
assert manifesto["target_triple"] == "x86_64-unknown-linux-gnu", manifesto
assert manifesto["target_label"] == "linux-x86_64", manifesto
assert manifesto["cargo_features"] == ["default"], manifesto
attesi = {
    "lume",
    "lume-process-watchdog",
    "lume-git-supervisor",
    "LICENSE.txt",
    "NOTICE.txt",
    "THIRD_PARTY_LICENSES.md",
    "THIRD_PARTY_NOTICES.md",
    "licenze/Inter-OFL.txt",
    "licenze/JetBrainsMono-OFL.txt",
    "licenze/material-icon-theme-MIT.txt",
    "licenze/catppuccin-vscode-icons-MIT.txt",
    "licenze/tiberius-ng-LICENSE-MIT.txt",
    "licenze/tiberius-ng-LICENSE-APACHE.txt",
    "licenze/aws-lc-sys-LICENSE.txt",
}
assert set(manifesto["files"]) == attesi, manifesto
atteso = hashlib.sha256(b"#!/bin/sh\nexit 0\n").hexdigest()
assert manifesto["files"]["lume"]["sha256"] == atteso, manifesto
sidecar = pathlib.Path(str(archivio) + ".MANIFEST.json").read_bytes()
assert sidecar == raw, "la copia del manifest differisce"
print("campi del manifest verificati")
PY

# 3. Overwrite: stage, archivio e manifest già presenti non si toccano.
if release_finalizza_tar "$stage" "$archivio" 0.3.0 "$commit" \
    x86_64-unknown-linux-gnu linux-x86_64 default >"$lavoro/overwrite.log" 2>&1; then
    fallisci "stage e archivio esistenti sovrascritti"
fi
grep -q 'output già esistente' "$lavoro/overwrite.log" ||
    fallisci "messaggio overwrite assente (manifest/stage)"
if release_rifiuta_esistenti "$archivio.sha256" "$archivio.MANIFEST.json" \
    >"$lavoro/overwrite2.log" 2>&1; then
    fallisci "checksum o manifest esistenti accettati"
fi
grep -q 'output già esistente' "$lavoro/overwrite2.log" ||
    fallisci "messaggio overwrite assente (checksum/manifest)"

# 4. Manifest e commit: contenuto alterato e SHA diverso vengono rifiutati.
manomesso="$lavoro/manomesso"
mkdir "$manomesso"
tar -C "$manomesso" -xzf "$archivio"
printf 'alterato\n' >> "$manomesso/$nome/lume"
tar -C "$manomesso" -czf "$manomesso/$nome.tar.gz" "$nome"
cp "$archivio.MANIFEST.json" "$manomesso/$nome.tar.gz.MANIFEST.json"
release_sha256_nome "$manomesso/$nome.tar.gz" > "$manomesso/$nome.tar.gz.sha256"
if release_verifica_manifest "$manomesso/$nome.tar.gz" "$commit" \
    >"$lavoro/manomesso.log" 2>&1; then
    fallisci "contenuto alterato accettato"
fi
grep -q 'sha256/size diversi' "$lavoro/manomesso.log" ||
    fallisci "il rifiuto non nomina gli hash del contenuto"
if release_verifica_manifest "$archivio" \
    0000000000000000000000000000000000000000 >"$lavoro/commit.log" 2>&1; then
    fallisci "commit diverso accettato"
fi
grep -q 'diverso da quello atteso' "$lavoro/commit.log" ||
    fallisci "il rifiuto non nomina il commit"

# 5. Lo stesso verificatore copre il manifest dentro lo zip Windows.
zip="$lavoro/artifacts/lume-0.3.0-windows-x86_64-$sha12.zip"
python3 - "$zip" "$commit" <<'PY'
import datetime
import hashlib
import json
import pathlib
import sys
import zipfile

zip_path = pathlib.Path(sys.argv[1])
commit = sys.argv[2]
file = {
    "lume.exe": b"MZ finto eseguibile\n",
    "servers/test.txt": b"server finto\n",
}
manifesto = {
    "schema": 1,
    "package": "lume",
    "version": "0.3.0",
    "commit": commit,
    "build_date_utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "rustc": "rustc 1.96.0 (fake release ingressi)",
    "cargo": "cargo 1.96.0 (fake release ingressi)",
    "target_triple": "x86_64-pc-windows-msvc",
    "target_label": "windows-x86_64",
    "cargo_features": ["default"],
    "files": {
        nome: {"sha256": hashlib.sha256(data).hexdigest(), "size": len(data)}
        for nome, data in file.items()
    },
}
raw = (json.dumps(manifesto, indent=2, sort_keys=True) + "\n").encode("utf-8")
with zipfile.ZipFile(zip_path, "x", zipfile.ZIP_DEFLATED) as archivio:
    for nome, data in file.items():
        archivio.writestr(f"Lume/{nome}", data)
    archivio.writestr("Lume/MANIFEST.json", raw)
pathlib.Path(str(zip_path) + ".MANIFEST.json").write_bytes(raw)
digest = hashlib.sha256(zip_path.read_bytes()).hexdigest()
pathlib.Path(str(zip_path) + ".sha256").write_text(
    f"{digest}  {zip_path.name}\n", encoding="utf-8"
)
PY
release_verifica_manifest "$zip" "$commit" >"$lavoro/zip.log" 2>&1 ||
    { cat "$lavoro/zip.log" >&2; fallisci "manifest dello zip rifiutato"; }

echo 'ingresso release verificato: albero pulito, nome con sha12, manifest e overwrite'
