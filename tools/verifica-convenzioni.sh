#!/usr/bin/env bash
# Convenzioni del codice che una macchina sa controllare (CONTRIBUTING.md,
# «Nomi e marcatori»). Esce con 1 e stampa ogni violazione.
# Uso: tools/verifica-convenzioni.sh [radice]   (default: la radice del repo)
set -u
radice="${1:-$(git rev-parse --show-toplevel)}"
esito=0

# Esamina anche sorgenti nuovi non ancora tracciati nel working tree.
mapfile -d '' files < <(find "$radice/crates" -type f -name '*.rs' -print0 | sort -z)

# 1. Il marcatore di attenzione nei commenti è la parola «ATTENZIONE:».
avviso=$(printf '\xe2\x9a\xa0\xef\xb8\x8f')
for file in "${files[@]}"; do
  if grep -nHF "$avviso" "$file"; then
    echo "^ usare «ATTENZIONE:» al posto dell'emoji (tools/uniforma-marcatori.sh)"
    esito=1
  fi
done

# 2. Nessun test legge i sorgenti: si prova il comportamento. Unica eccezione:
#    i test delle guide (crates/*/tests/guida.rs) che controllano che la
#    documentazione citi il codice vero; guardano la documentazione, non la logica.
for file in "${files[@]}"; do
  [[ "$file" == */tests/guida.rs ]] && continue
  if grep -nHE 'include_str!\("[^"]*\.rs"\)' "$file"; then
    echo "^ un test non deve leggere un file .rs: provare il comportamento"
    esito=1
  fi
done

# 3. Le tre «ricerche» dichiarano in testa di non essere le altre due.
for file in crates/lume-core/src/search.rs crates/lume-app/src/search.rs \
  crates/lume-app/src/tools/search_tool.rs; do
  if ! head -n 8 "$radice/$file" | grep -q 'Da non confondere'; then
    echo "$file: manca «Da non confondere» nelle prime 8 righe"
    esito=1
  fi
done

# 4. Nomi dei file: minuscolo con underscore; i binari in src/bin/ possono
#    avere anche il trattino, come il nome dell'eseguibile.
for file in "${files[@]}"; do
  nome=${file##*/}
  cartella=${file%/*}
  if [[ "$nome" =~ ^[a-z0-9_]+\.rs$ ]]; then
    continue
  fi
  if [[ "$cartella" == */src/bin && "$nome" =~ ^[a-z0-9_-]+\.rs$ ]]; then
    continue
  fi
  echo "$file: nome del file fuori convenzione"
  esito=1
done

if [ "$esito" -eq 0 ]; then echo "ok: convenzioni rispettate"; fi
exit "$esito"
