#!/usr/bin/env bash
# Verifica con un cargo finto che il packaging inoltri le rimappature senza
# perdere RUSTFLAGS, CARGO_ENCODED_RUSTFLAGS o CFLAGS preesistenti.
set -euo pipefail

radice="$(cd "$(dirname "$0")/.." && pwd)"
lavoro="$(mktemp -d)"
tag="test-path-remap-$(date +%s)-$$"
versione="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$radice/Cargo.toml" | head -n 1)"
sha12="$(git -C "$radice" rev-parse HEAD | cut -c1-12)"
nome_encoded="lume-$versione-$tag-encoded-$sha12"
nome_plain="lume-$versione-$tag-plain-$sha12"
uscite=(
    "$radice/artifacts/$nome_encoded"
    "$radice/artifacts/$nome_plain"
    "$radice/artifacts/$nome_encoded.tar.gz"
    "$radice/artifacts/$nome_encoded.tar.gz.sha256"
    "$radice/artifacts/$nome_encoded.tar.gz.MANIFEST.json"
    "$radice/artifacts/$nome_plain.tar.gz"
    "$radice/artifacts/$nome_plain.tar.gz.sha256"
    "$radice/artifacts/$nome_plain.tar.gz.MANIFEST.json"
)
trap 'rm -rf "$lavoro" "${uscite[@]}"' EXIT

mkdir -p "$lavoro/bin"
cat > "$lavoro/bin/rustc" <<'SH'
#!/usr/bin/env bash
case "${1:-}" in
    -vV) printf 'host: %s\n' "${LUME_FAKE_HOST_TRIPLE:-x86_64-unknown-linux-gnu}" ;;
    --version) printf 'rustc 1.96.0 (fake path remap)\n' ;;
    *) printf 'argomento rustc inatteso: %s\n' "${1:-}" >&2; exit 1 ;;
esac
SH
cat > "$lavoro/bin/cargo" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
    --version) printf 'cargo 1.96.0 (fake path remap)\n'; exit 0 ;;
    build) ;;
    *) printf 'argomento cargo inatteso: %s\n' "${1:-}" >&2; exit 1 ;;
esac
mkdir -p "$LUME_CAPTURE_DIR"
printf '%s' "${CARGO_ENCODED_RUSTFLAGS-}" > "$LUME_CAPTURE_DIR/encoded"
printf '%s' "${RUSTFLAGS-}" > "$LUME_CAPTURE_DIR/plain"
printf '%s' "${CFLAGS-}" > "$LUME_CAPTURE_DIR/cflags"
printf '%s' "${CC_SHELL_ESCAPED_FLAGS-}" > "$LUME_CAPTURE_DIR/cc-shell-escaped"
target="${CARGO_TARGET_DIR:?CARGO_TARGET_DIR richiesto}"
mkdir -p "$target/release"
printf '#!/bin/sh\nexit 0\n' > "$target/release/lume"
printf '#!/bin/sh\nexit 0\n' > "$target/release/lume-process-watchdog"
printf '#!/bin/sh\necho "Lume Git supervisor: missing supervisor data" >&2\nexit 127\n' > "$target/release/lume-git-supervisor"
chmod +x "$target/release/lume" "$target/release/lume-process-watchdog" "$target/release/lume-git-supervisor"
SH
chmod +x "$lavoro/bin/cargo" "$lavoro/bin/rustc"

export PATH="$lavoro/bin:$PATH"
export CARGO_HOME="$lavoro/cargo home"
export RUSTUP_HOME="$lavoro/rustup home"
export CFLAGS='-DKEEP_CFLAG'
export CARGO_TARGET_DIR="$lavoro/target with spaces"

export LUME_CAPTURE_DIR="$lavoro/encoded-capture"
export CARGO_ENCODED_RUSTFLAGS='--cfg=keep_encoded'
unset RUSTFLAGS || true
if ! bash "$radice/tools/pacchetto_unix.sh" "$tag-encoded" > "$lavoro/encoded.log" 2>&1; then
    cat "$lavoro/encoded.log" >&2
    exit 1
fi

export LUME_CAPTURE_DIR="$lavoro/plain-capture"
unset CARGO_ENCODED_RUSTFLAGS || true
export RUSTFLAGS='--cfg=keep_plain'
if ! bash "$radice/tools/pacchetto_unix.sh" "$tag-plain" > "$lavoro/plain.log" 2>&1; then
    cat "$lavoro/plain.log" >&2
    exit 1
fi

python3 - "$lavoro" "$radice" <<'PY'
import pathlib
import shlex
import sys

work = pathlib.Path(sys.argv[1])
root = pathlib.Path(sys.argv[2])
cargo_home = str(work / "cargo home")
rustup_home = str(work / "rustup home")
repo = str(root)

encoded_dir = work / "encoded-capture"
encoded = (encoded_dir / "encoded").read_text()
assert encoded.split("\x1f")[0] == "--cfg=keep_encoded", encoded
encoded_tokens = encoded.split("\x1f")
for expected in (
    f"--remap-path-prefix={cargo_home}=/cargo",
    f"--remap-path-prefix={rustup_home}=/rustup",
    f"--remap-path-prefix={repo}=/lume",
):
    assert expected in encoded_tokens, (expected, encoded_tokens)
assert (encoded_dir / "plain").read_text() == "", "RUSTFLAGS inatteso nel caso encoded"

plain_dir = work / "plain-capture"
plain_encoded = (plain_dir / "encoded").read_text().split("\x1f")
assert plain_encoded[0] == "--cfg=keep_plain", plain_encoded
for expected in (
    f"--remap-path-prefix={cargo_home}=/cargo",
    f"--remap-path-prefix={rustup_home}=/rustup",
    f"--remap-path-prefix={repo}=/lume",
):
    assert expected in plain_encoded, (expected, plain_encoded)
assert (plain_dir / "plain").read_text() == "", "RUSTFLAGS non convertito nel formato encoded"

for capture in (encoded_dir, plain_dir):
    cflags = shlex.split((capture / "cflags").read_text())
    assert "-DKEEP_CFLAG" in cflags, cflags
    for expected in (
        f"-ffile-prefix-map={cargo_home}=/cargo",
        f"-ffile-prefix-map={repo}=/lume",
    ):
        assert expected in cflags, (expected, cflags)
    assert (capture / "cc-shell-escaped").read_text() == "1"

print("rimappature Rust/C e flag utente verificati per entrambi i modi RUSTFLAGS")
PY
