# Lume su Linux

Estrai il pacchetto in una cartella stabile e avvia `./avvia-lume.sh`.
Puoi passare un file come argomento. Per aprire un progetto usa File → Apri… nell’app.

Per aggiungere Lume al menu applicazioni, esegui una volta:

```sh
./installa-launcher.sh
```

Il launcher e l'icona vengono installati soltanto per il tuo utente.
Il desktop associa l'icona alla finestra tramite `io.github.mak1jk.Lume`.
Se sposti il pacchetto, riesegui lo script dalla nuova cartella.
Per rimuovere la voce dal menu, elimina
`~/.local/share/applications/io.github.mak1jk.Lume.desktop` e
`~/.local/share/icons/hicolor/scalable/apps/io.github.mak1jk.Lume.svg`
(o gli stessi file sotto `$XDG_DATA_HOME`, se configurato).

La finestra e le risorse Windows usano la stessa L azzurra illuminata
da una lampada dell’icona vettoriale. L'icona delle impostazioni conserva la sua
funzione abituale.
