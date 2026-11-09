# Lume 0.4.2 alpha — note di rilascio

Versione sulla [0.4.1](note-di-rilascio-0.4.1.md), per Windows x64 e Linux
x86_64. Lo stato resta **alpha qualificata su Linux**: le funzioni nuove di
questa versione sono provate solo su Linux.

## Che cosa cambia

- **Debugger con il flusso del codice.** Pannello «Debug» (barra sinistra,
  F5): al primo arresto mostra il **percorso di esecuzione a cascata**, dalle
  chiamate più esterne fino al punto di arresto, con le righe di codice
  eseguite a ogni livello e la storia degli arresti da scorrere. Accanto: il
  sorgente del passo scelto e le schede Input, Variabili, Output, Valuta e
  Breakpoint. Continua (F5), Passo (F10), Entra, Esci, Ferma (Maiusc+F5).
  Nell'editor: breakpoint con un clic sul numero di riga (dopo aver aperto il
  pannello Debug nel progetto) o con F9, riga corrente e righe già fermate.
  Usa **GNU gdb 14 o successivo** già installato (`gdb -i dap`): Lume non
  scarica adattatori. Avviare il debug chiede fiducia nel progetto, una volta
  per avvio di Lume.
- **Esecuzione stile IntelliJ.** Nella barra del titolo: configurazione ▾,
  ▶ Esegui, 🐞 Debug, ■ Ferma. «File corrente» esegue Python, Node, Java a
  file singolo e C (che si può anche debuggare: Lume lo compila con `-g`).
  Ogni esecuzione apre una scheda nel Terminale, con Riavvia che ripete il
  comando. ▶ e 🐞 compaiono solo se sul sistema c'è lo strumento che serve.
- **Configurazioni di esecuzione.** Finestra con elenco per tipo
  (Applicazione, Maven, Comando) e modulo: programma e argomenti, oppure goal
  Maven, profili e opzioni; cartella, variabili d'ambiente. Salva in
  `.lume/launch.toml`, che si può anche modificare a mano.
- **Maven.** Pannello «Maven» nella barra destra, quando il progetto ha un
  `pom.xml`: ciclo di vita eseguibile con un clic, plugin (con `exec:java`),
  profili da attivare, moduli. Lume legge solo il `pom.xml` e non avvia
  Maven finché non lo chiedi; usa `./mvnw` del progetto o `mvn` del sistema.
  Nessun componente Java pesante: niente language server Java né indici.
- **Log della grafica.** GPU e backend scelti e gli errori di presentazione
  dei frame finiscono in `<dati>/logs/grafica.log`, per diagnosticare lo
  sfarfallio su Windows.

## Limiti di questa versione

- Debugger, widget Esegui, configurazioni e pannello Maven sono provati su
  **Linux** (gdb 17.1, Maven 3.9.16, JDK 21). Su Windows compilano e passano i
  test automatici, ma l'uso reale non è collaudato; su Windows serve un gdb
  con supporto DAP.
- Niente debug Java: il pulsante 🐞 compare solo per programmi nativi con gdb.
- Il pannello Maven non mostra l'albero delle dipendenze risolte.
- Salvare dalla finestra delle configurazioni riscrive `.lume/launch.toml` e
  non conserva i commenti scritti a mano.
- I breakpoint non si conservano fra un avvio di Lume e l'altro.
- Gli altri limiti sono quelli delle [note della 0.4.0](note-di-rilascio-0.4.0.md#limiti-noti-di-questa-alpha);
  lo sfarfallio dello scroll su Windows resta non riprodotto.
