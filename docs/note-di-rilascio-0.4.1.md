# Lume 0.4.1 alpha — note di rilascio

Versione di correzione sulla [0.4.0](note-di-rilascio-0.4.0.md), per Windows
x64 e Linux x86_64. Lo stato resta **alpha qualificata su Linux**; i limiti
noti sono quelli della 0.4.0.

## Che cosa cambia

- **Windows senza Visual C++ Redistributable.** La 0.4.0 importava
  `VCRUNTIME140.dll` e su un Windows che non l'aveva non partiva. Ora il
  runtime C è dentro `lume.exe`.
- **Aggiornamenti dal menu.** «Aiuto → Scarica aggiornamento…» controlla la
  release più recente su GitHub e, se è nuova, scarica il pacchetto della
  tua piattaforma nella cartella Download (o nella home, se il sistema non
  ne indica una). Prima di dirlo pronto verifica
  che dimensione e sha256 coincidano con quelli pubblicati; se qualcosa non
  torna scarta il file e lo dice. Lume **non** estrae il pacchetto e non
  sostituisce il proprio eseguibile: chiudi Lume, estrai lo zip e avvia la
  nuova versione. È una scelta voluta: su un PC con un EDR aziendale un
  programma che riscrive sé stesso viene bloccato o segnalato.
- **Controllo aggiornamenti corretto.** «Controlla aggiornamenti…» cercava
  pacchetti con un nome che le release non usano e quindi non li trovava.
- **Scroll dell'editor.** Le righe visibili si disegnano in un unico
  livello e i colori della sintassi arrivano prima che servano. Su Linux,
  con il renderer software, la CPU per frame scende di circa un quarto.

## Che cosa non cambia

- Lo **sfarfallio durante lo scroll veloce su Windows**, la prima volta che
  si apre un file, non è stato riprodotto: né su Linux né in una VM Windows
  con GPU software. Le modifiche allo scroll tolgono lavoro inutile ma non
  sono una correzione dimostrata di quel difetto.
- Il pacchetto Windows non è firmato.
- macOS non è in questa release.
- Gli altri limiti sono quelli elencati nelle
  [note della 0.4.0](note-di-rilascio-0.4.0.md#limiti-noti-di-questa-alpha).
