#!/usr/bin/env bash
# Registra il pacchetto estratto nel menu applicazioni dell'utente.
set -euo pipefail
radice="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
[ -x "$radice/avvia-lume.sh" ] && [ -f "$radice/lume.svg" ] || {
    echo "Esegui questo script dalla cartella del pacchetto Linux estratto." >&2
    exit 1
}
dati="${XDG_DATA_HOME:-$HOME/.local/share}"
case "$dati" in /*) ;; *) echo "XDG_DATA_HOME deve essere un percorso assoluto." >&2; exit 1 ;; esac
# Un valore Exec è prima decodificato come stringa desktop, poi come
# argomento tra doppi apici: i backslash richiedono due livelli di escape.
escape_exec() {
    local valore="$1"
    case "$valore" in *$'\n'*|*$'\r'*) echo "Percorso con newline non supportato." >&2; return 1 ;; esac
    valore="${valore//\\/\\\\\\\\}"
    valore="${valore//\"/\\\\\\\"}"
    valore="${valore//\$/\\\\\$}"
    valore="${valore//\`/\\\\\`}"
    valore="${valore//%/%%}"
    printf '%s' "$valore"
}
comando="$(escape_exec "$radice/avvia-lume.sh")"
mkdir -p "$dati/applications" "$dati/icons/hicolor/scalable/apps"
install -m 644 "$radice/lume.svg" "$dati/icons/hicolor/scalable/apps/io.github.mak1jk.Lume.svg"
file="$dati/applications/io.github.mak1jk.Lume.desktop"
[ ! -L "$file" ] || { echo "Launcher esistente collegato: installazione rifiutata." >&2; exit 1; }
umask 022
cat > "$file" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Lume
Comment=Editor di codice
Exec="$comando" %F
Icon=io.github.mak1jk.Lume
Terminal=false
Categories=Development;IDE;TextEditor;
StartupWMClass=io.github.mak1jk.Lume
StartupNotify=true
DESKTOP
chmod 644 "$file"
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$dati/applications" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$dati/icons/hicolor" >/dev/null 2>&1 || true
fi
printf 'Launcher installato: %s\nConserva il pacchetto in: %s\n' "$file" "$radice"
