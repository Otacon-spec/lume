#!/usr/bin/env python3
"""Verifica il MANIFEST.json di un artefatto Lume e il commit dichiarato.

Uso: verifica_manifest_artefatto.py <artefatto.tar.gz|artefatto.zip> <commit>

Controlla che il manifest sia nella radice del pacchetto, che l'elenco dei
file e i loro sha256 coincidano con il contenuto reale, che la copia
<artefatto>.MANIFEST.json sia identica e che lo SHA nel nome e nel manifest
sia quello del commit atteso. Non estrae nulla su disco.
"""

import datetime
import hashlib
import json
import pathlib
import re
import stat
import sys
import tarfile
import zipfile


def errore(messaggio):
    print(f"manifest non valido: {messaggio}", file=sys.stderr)
    raise SystemExit(1)


def sha256_file(percorso):
    digest = hashlib.sha256()
    with open(percorso, "rb") as handle:
        for blocco in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(blocco)
    return digest.hexdigest()


def sha256_stream(stream):
    digest = hashlib.sha256()
    dimensione = 0
    for blocco in iter(lambda: stream.read(1024 * 1024), b""):
        digest.update(blocco)
        dimensione += len(blocco)
    return digest.hexdigest(), dimensione


def relativo_valido(nome):
    if not nome or nome.startswith("/"):
        return False
    parti = nome.split("/")
    return all(parte not in ("", ".", "..") for parte in parti)


def radice_unica(nomi):
    radici = set()
    for nome in nomi:
        senza_barra = nome.rstrip("/")
        if not senza_barra:
            continue
        if not relativo_valido(senza_barra):
            errore(f"percorso non valido nell'artefatto: {nome}")
        radici.add(senza_barra.split("/")[0])
    if len(radici) != 1:
        errore(f"attesa una sola cartella radice, trovate: {sorted(radici)}")
    return radici.pop()


def leggi_tar(percorso):
    with tarfile.open(percorso, "r:gz") as archivio:
        membri = archivio.getmembers()
        radice = radice_unica([m.name for m in membri])
        manifest_name = f"{radice}/MANIFEST.json"
        raw = None
        for membro in membri:
            if membro.name.rstrip("/") == manifest_name:
                if not membro.isfile():
                    errore(f"MANIFEST.json non è un file regolare: {manifest_name}")
                estratto = archivio.extractfile(membro)
                if estratto is None:
                    errore(f"impossibile leggere {manifest_name}")
                raw = estratto.read()
                break
        if raw is None:
            errore(f"manca {manifest_name} nella radice del pacchetto")
        file_reali = {}
        for membro in membri:
            if membro.isdir():
                continue
            nome = membro.name.rstrip("/")
            if nome == manifest_name:
                continue
            if not nome.startswith(radice + "/"):
                errore(f"file fuori dalla radice {radice}: {membro.name}")
            relativo = nome[len(radice) + 1 :]
            if membro.issym() or membro.islnk() or not membro.isfile():
                errore(f"voce non regolare nell'archivio: {membro.name}")
            estratto = archivio.extractfile(membro)
            if estratto is None:
                errore(f"impossibile leggere {membro.name}")
            digest, dimensione = sha256_stream(estratto)
            if relativo in file_reali:
                errore(f"file duplicato nell'archivio: {relativo}")
            file_reali[relativo] = (digest, dimensione)
    return raw, file_reali


def leggi_zip(percorso):
    with zipfile.ZipFile(percorso) as archivio:
        voci = archivio.infolist()
        radice = radice_unica([voce.filename for voce in voci])
        manifest_name = f"{radice}/MANIFEST.json"
        raw = None
        file_reali = {}
        for voce in voci:
            nome = voce.filename.rstrip("/")
            if nome == manifest_name:
                if not voce.is_dir():
                    raw = archivio.read(voce)
                continue
            if voce.is_dir():
                continue
            if stat.S_ISLNK(voce.external_attr >> 16):
                errore(f"collegamento simbolico nell'archivio: {voce.filename}")
            if not nome.startswith(radice + "/"):
                errore(f"file fuori dalla radice {radice}: {voce.filename}")
            relativo = nome[len(radice) + 1 :]
            if relativo in file_reali:
                errore(f"file duplicato nell'archivio: {relativo}")
            with archivio.open(voce) as stream:
                digest, dimensione = sha256_stream(stream)
            file_reali[relativo] = (digest, dimensione)
        if raw is None:
            errore(f"manca {manifest_name} nella radice del pacchetto")
    return raw, file_reali


def valida_testo(documento, campo):
    valore = documento.get(campo)
    if not isinstance(valore, str) or not valore.strip():
        errore(f"campo «{campo}» mancante o vuoto")
    return valore


def valida_manifest(raw, artefatto, commit):
    try:
        documento = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as errore_json:
        errore(f"JSON illeggibile: {errore_json}")
    if not isinstance(documento, dict):
        errore("la radice del manifest non è un oggetto JSON")
    if documento.get("schema") != 1:
        errore(f"schema inatteso: {documento.get('schema')!r}")
    if documento.get("package") != "lume":
        errore(f"pacchetto inatteso: {documento.get('package')!r}")

    versione = valida_testo(documento, "version")
    commit_manifest = valida_testo(documento, "commit")
    data_utc = valida_testo(documento, "build_date_utc")
    valida_testo(documento, "rustc")
    valida_testo(documento, "cargo")
    triple = valida_testo(documento, "target_triple")
    etichetta = valida_testo(documento, "target_label")

    if not re.fullmatch(r"[0-9a-f]{40}", commit_manifest):
        errore(f"commit non valido nel manifest: {commit_manifest}")
    if commit_manifest != commit:
        errore(
            f"commit del manifest {commit_manifest} diverso da quello atteso {commit}"
        )
    try:
        datetime.datetime.strptime(data_utc, "%Y-%m-%dT%H:%M:%SZ")
    except ValueError:
        errore(f"build_date_utc non è una data UTC ISO-8601: {data_utc}")

    features = documento.get("cargo_features")
    if (
        not isinstance(features, list)
        or not features
        or not all(isinstance(f, str) and f.strip() for f in features)
    ):
        errore("cargo_features deve essere un elenco non vuoto di stringhe")

    nome = pathlib.Path(artefatto).name
    if nome.endswith(".tar.gz"):
        estensione = "tar.gz"
    elif nome.endswith(".zip"):
        estensione = "zip"
    else:
        errore(f"estensione non riconosciuta: {nome}")
    nome_atteso = f"lume-{versione}-{etichetta}-{commit[:12]}.{estensione}"
    if nome != nome_atteso:
        errore(
            f"nome artefatto inatteso: {nome}, "
            f"atteso lume-<versione>-<target>-<sha12> cioè {nome_atteso}"
        )
    if estensione == "tar.gz" and triple == "":
        errore("target_triple vuoto")

    file_manifest = documento.get("files")
    if not isinstance(file_manifest, dict) or not file_manifest:
        errore("files deve essere un oggetto non vuoto")
    normalizzati = {}
    for relativo, dettagli in file_manifest.items():
        if not relativo_valido(relativo) or relativo == "MANIFEST.json":
            errore(f"percorso non valido in files: {relativo!r}")
        if not isinstance(dettagli, dict):
            errore(f"dettagli non validi per {relativo}")
        digest = dettagli.get("sha256")
        dimensione = dettagli.get("size")
        if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
            errore(f"sha256 non valido per {relativo}")
        if not isinstance(dimensione, int) or dimensione < 0:
            errore(f"size non valida per {relativo}")
        normalizzati[relativo] = (digest, dimensione)
    return normalizzati, commit_manifest, versione, etichetta


def confronta(file_manifest, file_reali):
    mancanti = sorted(set(file_manifest) - set(file_reali))
    extra = sorted(set(file_reali) - set(file_manifest))
    if mancanti:
        errore(f"file elencati nel manifest ma assenti: {', '.join(mancanti[:5])}")
    if extra:
        errore(f"file presenti ma non elencati nel manifest: {', '.join(extra[:5])}")
    diversi = [
        relativo
        for relativo, (digest, dimensione) in file_manifest.items()
        if file_reali[relativo] != (digest, dimensione)
    ]
    if diversi:
        errore(f"sha256/size diversi dal manifest: {', '.join(sorted(diversi)[:5])}")


def main():
    if len(sys.argv) != 3:
        print(
            "uso: verifica_manifest_artefatto.py <artefatto.tar.gz|.zip> <commit>",
            file=sys.stderr,
        )
        return 2
    artefatto = pathlib.Path(sys.argv[1])
    commit = sys.argv[2].lower()
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        errore(f"commit atteso non valido: {sys.argv[2]}")
    if not artefatto.is_file():
        errore(f"artefatto inesistente: {artefatto}")

    checksum_path = pathlib.Path(str(artefatto) + ".sha256")
    if not checksum_path.is_file():
        errore(f"manca il checksum accanto all'artefatto: {checksum_path}")
    parti = checksum_path.read_text(encoding="utf-8").split()
    if not parti or parti[0].lower() != sha256_file(artefatto):
        errore(f"checksum {checksum_path} non corrisponde all'artefatto")
    if len(parti) > 1 and parti[1] != artefatto.name:
        errore(f"checksum {checksum_path} nomina un altro file: {parti[1]}")

    if artefatto.name.endswith(".tar.gz"):
        raw, file_reali = leggi_tar(artefatto)
    elif artefatto.name.endswith(".zip"):
        raw, file_reali = leggi_zip(artefatto)
    else:
        errore(f"estensione non riconosciuta: {artefatto.name}")

    sidecar = pathlib.Path(str(artefatto) + ".MANIFEST.json")
    if not sidecar.is_file():
        errore(f"manca la copia del manifest accanto all'artefatto: {sidecar}")
    if sidecar.read_bytes() != raw:
        errore(f"la copia {sidecar.name} differisce dal manifest nell'artefatto")

    file_manifest, _, versione, etichetta = valida_manifest(raw, artefatto, commit)
    confronta(file_manifest, file_reali)
    print(
        f"manifest valido: {artefatto.name} (versione {versione}, target {etichetta}, "
        f"commit {commit})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
