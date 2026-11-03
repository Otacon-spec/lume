#!/usr/bin/env bash
# Crea l'archivio Unix con Lume, il watchdog e tutte le licenze richieste.
# Uso: bash tools/pacchetto_unix.sh <etichetta>, es. linux-x86_64.
#
# L'artefatto è lume-<versione>-<etichetta>-<sha12>.tar.gz e nasce solo da un
# albero Git pulito. Accanto scrive .sha256 e la copia di MANIFEST.json; non
# sovrascrive output già esistenti.
set -euo pipefail

sorgente="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=tools/lib_release.sh
. "$sorgente/lib_release.sh"

etichetta="${1:?uso: pacchetto_unix.sh <etichetta>}"
release_valida_etichetta "$etichetta" || exit 1

radice="$(cd "$sorgente/.." && pwd)"
cd "$radice"
release_require_clean_tree "$radice"
versione="$(release_version "$radice")"
commit="$(release_commit "$radice")"
sha12="$(printf '%s' "$commit" | cut -c1-12)"
triple="$(release_target_triple)"
features="$(release_cargo_features)"
nome="$(release_artifact_basename "$versione" "$etichetta" "$sha12")"

# Rimappa i path di sorgente e toolchain incorporati da rustc nei binari.
# CARGO_ENCODED_RUSTFLAGS è necessario per passare i path con spazi: Cargo
# divide RUSTFLAGS sugli spazi senza interpretare quoting shell.
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
rust_flags=(
    "--remap-path-prefix=$cargo_home=/cargo"
    "--remap-path-prefix=$rustup_home=/rustup"
    "--remap-path-prefix=$radice=/lume"
)

if [[ "${CARGO_ENCODED_RUSTFLAGS+x}" == x ]]; then
    CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS%$'\x1f'}"
else
    # Cargo interpreta RUSTFLAGS come una lista separata da whitespace, non
    # come una command line shell; manteniamo quindi gli stessi token.
    read -r -a rust_flags_utente <<< "${RUSTFLAGS:-}"
    CARGO_ENCODED_RUSTFLAGS=""
    for flag in "${rust_flags_utente[@]}"; do
        CARGO_ENCODED_RUSTFLAGS+="${CARGO_ENCODED_RUSTFLAGS:+$'\x1f'}$flag"
    done
    unset RUSTFLAGS
fi
for flag in "${rust_flags[@]}"; do
    CARGO_ENCODED_RUSTFLAGS+="${CARGO_ENCODED_RUSTFLAGS:+$'\x1f'}$flag"
done
export CARGO_ENCODED_RUSTFLAGS

# Le dipendenze native includono __FILE__ (per esempio gli assert di tree-sitter).
# cc-rs legge CFLAGS tramite shell parsing quando CC_SHELL_ESCAPED_FLAGS=1.
export CC_SHELL_ESCAPED_FLAGS=1
CFLAGS="${CFLAGS:-}"
c_flags=(
    "-ffile-prefix-map=$cargo_home=/cargo"
    "-ffile-prefix-map=$radice=/lume"
)
for flag in "${c_flags[@]}"; do
    if [[ -n "$CFLAGS" ]]; then
        CFLAGS+=" "
    fi
    escaped="${flag//\\/\\\\}"
    escaped="${escaped//\"/\\\"}"
    CFLAGS+="\"$escaped\""
done
export CFLAGS

target="${CARGO_TARGET_DIR:-$radice/target}"
case "$target" in
    /*) ;;
    *) target="$radice/$target" ;;
esac
uscita="$radice/artifacts"
stage="$uscita/$nome"
archivio="$uscita/$nome.tar.gz"

if [ -L "$uscita" ]; then
    echo "rifiuto output artifacts che è un collegamento simbolico" >&2
    exit 1
fi
release_rifiuta_esistenti "$stage" "$archivio" "$archivio.sha256" \
    "$archivio.MANIFEST.json" || exit 1

cargo build --release --locked --manifest-path "$radice/Cargo.toml" \
    --bin lume --bin lume-process-watchdog --bin lume-git-supervisor

mkdir -p "$uscita"
# mkdir senza -p: se lo stage compare dopo il controllo, non lo si riusa.
mkdir "$stage"
mkdir "$stage/licenze"
install -m 755 "$target/release/lume" "$stage/lume"
install -m 755 "$target/release/lume-process-watchdog" "$stage/lume-process-watchdog"
install -m 755 "$target/release/lume-git-supervisor" "$stage/lume-git-supervisor"

install -m 755 "$radice/tools/avvia_lume_unix.sh" "$stage/avvia-lume.sh"
# Il nome desktop deve coincidere con application_id della finestra.
case "$etichetta" in
    linux-*)
        install -m 644 "$radice/assets/app/lume.svg" "$stage/lume.svg"
        install -m 644 "$radice/assets/app/README-LINUX.md" "$stage/README-LINUX.md"
        install -m 755 "$radice/tools/installa_launcher_linux.sh" "$stage/installa-launcher.sh"
        ;;
esac

cp "$radice/LICENSE" "$stage/LICENSE.txt"
cp "$radice/NOTICE" "$stage/NOTICE.txt"
cp "$radice/THIRD_PARTY_LICENSES.md" "$stage/THIRD_PARTY_LICENSES.md"
cp "$radice/THIRD_PARTY_NOTICES.md" "$stage/THIRD_PARTY_NOTICES.md"
cp "$radice/assets/fonts/LICENSE-Inter.txt" "$stage/licenze/Inter-OFL.txt"
cp "$radice/assets/fonts/LICENSE-JetBrainsMono.txt" "$stage/licenze/JetBrainsMono-OFL.txt"
cp "$radice/crates/lume-icons/assets/material/LICENSE" "$stage/licenze/material-icon-theme-MIT.txt"
cp "$radice/crates/lume-icons/assets/catppuccin-mocha/LICENSE" "$stage/licenze/catppuccin-vscode-icons-MIT.txt"
cp "$radice/vendor/tiberius-ng/LICENSE-MIT.txt" "$stage/licenze/tiberius-ng-LICENSE-MIT.txt"
cp "$radice/vendor/tiberius-ng/LICENSE-APACHE.txt" "$stage/licenze/tiberius-ng-LICENSE-APACHE.txt"
cp "$radice/crates/lume-language-servers/assets/licenses/aws-lc-sys-LICENSE.txt" \
    "$stage/licenze/aws-lc-sys-LICENSE.txt"

release_finalizza_tar "$stage" "$archivio" "$versione" "$commit" "$triple" \
    "$etichetta" "$features" >/dev/null
echo "scritto: $archivio"
