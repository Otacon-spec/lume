#!/usr/bin/env bash
# Sostituisce l'emoji di attenzione seguita da uno spazio con «ATTENZIONE: »
# in ogni sorgente Rust sotto crates/. Idempotente. Non tocca i glifi
# dell'interfaccia (U+26A0 senza selettore di variante).
# Uso: tools/uniforma-marcatori.sh [radice]
set -eu
radice="${1:-$(git rev-parse --show-toplevel)}"
avviso=$(printf '\xe2\x9a\xa0\xef\xb8\x8f')
while IFS= read -r -d '' file; do
  if grep -qF "$avviso" "$file"; then
    LC_ALL=C sed -i 's/\xE2\x9A\xA0\xEF\xB8\x8F /ATTENZIONE: /g' "$file"
    printf 'aggiornato: %s\n' "${file#"$radice"/}"
  fi
done < <(find "$radice/crates" -type f -name '*.rs' -print0 | sort -z)
