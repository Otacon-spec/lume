#!/usr/bin/env bash
# Ogni blocco `unsafe { ... }` in crates/**/*.rs deve avere sopra, senza righe
# vuote in mezzo, un commento `// SAFETY:`. Il controllo è testuale e vale su
# ogni piattaforma: clippy::undocumented_unsafe_blocks vede solo il codice
# compilato, e su Windows i blocchi `cfg(unix)` non li compila.
# Uso: tools/verifica-safety.sh [radice]   (default: la radice del repo)
set -u
radice="${1:-$(git rev-parse --show-toplevel)}"
esito=0
while IFS= read -r -d '' path; do
  file="${path#"$radice"/}"
  awk -v nome="$file" '
    function taglia(s) { sub(/^[ \t]+/, "", s); return s }
    { sub(/\r$/, ""); riga[NR] = $0 }
    END {
      for (i = 1; i <= NR; i++) {
        if (taglia(riga[i]) ~ /^\/\// || riga[i] !~ /(^|[^A-Za-z0-9_])unsafe[ \t]*\{/) continue
        s = i
        if (taglia(riga[i]) ~ /^unsafe/ && i > 1 && riga[i-1] ~ /(=|\()[ \t]*$/) s = i - 1
        ok = 0
        for (j = s - 1; j >= 1 && taglia(riga[j]) ~ /^\/\//; j--)
          if (riga[j] ~ /SAFETY:/) ok = 1
        if (!ok) { print nome ":" i ": unsafe senza // SAFETY:"; mancanti++ }
      }
      exit (mancanti > 0)
    }' "$radice/$file" || esito=1
done < <(find "$radice/crates" -type f -name '*.rs' -print0)
if [ "$esito" -eq 0 ]; then echo "ok: ogni blocco unsafe ha il suo SAFETY"; fi
exit "$esito"
