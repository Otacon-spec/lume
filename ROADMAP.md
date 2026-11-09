# Dopo la prima alpha

Questo documento dice che cosa Lume è oggi e in che ordine arriva il resto.
Non è un elenco di pulsanti promessi: un controllo entra nell'interfaccia solo
con un backend funzionante (regola zero-mock) e una voce è chiusa quando passa
i test ed è verificata sul binario distribuito.

Il punto di partenza è la **0.4.0 alpha, qualificata su Linux x86_64**: i
limiti di quella qualifica sono in «Limiti noti di questa alpha» del README e
nelle note di rilascio. Windows 10/11 x64 resta la piattaforma principale di
sviluppo, ma non è qualificata in questa release.

## 1. Affidabilità dei dati — nell'alpha, con limiti dichiarati

Fatto: salvataggi atomici per file, senza troncare l'originale prima del
commit; scritture fuori dal thread dell'interfaccia con revisioni esplicite;
un coordinatore unico per salvataggi, rinomine, Cestino e Git, con esiti
autenticati e nessun commit dopo un'autorizzazione revocata. Gli errori
parziali dichiarano quali file sono stati scritti; nessun rollback
immaginario.

Restano: la prova adversariale della finestra fra controllo e rename con uno
scrittore esterno; la matrice Windows (NTFS, FAT/exFAT, SMB, WSL, DACL,
readonly, perdita di alimentazione).

## 2. Operazioni Git — nell'alpha, con limiti dichiarati

Fatto: fetch annullabile con stato occupato e risposte stantie scartate;
push, cambio branch, stash apply/pop e merge con conflitti tramite il
programma `git`; pull con avanzamento rapido e merge pulito nativi; undo e
discard con conferme corrette.

Restano: autenticazione, TLS, credential helper e SSH verso un remote reale;
la prova nativa della raccolta di lettori e discendenti dei processi Git; il
rilascio della cache Git alla chiusura del pannello.

## 3. Prestazioni — da misurare

I budget del README (avvio < 100 ms, digitazione < 8 ms, 60 fps, riposo
< 150 MiB di private bytes) sono definiti su Windows release e non sono
misurati su questa release. Servono: una baseline Windows riproducibile con
il corpus dichiarato, le misure sul binario Linux di release, e la tabella
obiettivo/misura/scarto. Le misure Linux di oggi sono diagnostiche.

## 4. Editor e language server — nell'alpha, da qualificare

Fatto: completamento, hover, firma, diagnostica nel margine e nel pannello
Problemi, definizione, usi, rinomina, struttura, blame; folding,
multicursore, azioni rapide e formattazione.

Restano: rieseguire le prove native con un server reale sul binario
distribuito; un secondo server distinto per Java, Python o C#; il costo di
`didChange` sui file grandi; la raccolta di server morti e thread.

## 5. SQL — coperto dai test, non da un database reale

Fatto: console per Azure SQL/SQL Server e Oracle, TLS di default su Oracle,
interruzione della query dove il driver la supporta, password nel magazzino
credenziali del sistema.

Restano: prove contro istanze reali con account read-only e privilegi
dichiarati (TLS, CA aziendali, tipi LOB, streaming, cancellazione); più
result set Azure; i tool SQL del server MCP con i loro permessi e il consenso
per ogni query. Nessuno di questi è nella 0.4.0.

## 6. Manutenibilità — estrazione dei coordinatori dopo l'alpha

L'estrazione meccanica dei domini da `lume-app/src/app.rs` non è un
prerequisito della prima release e viene dopo. Sono già estratti il
coordinatore delle scritture, la worklist delle richieste dei tool e il ciclo
di vita delle funzionalità; le estrazioni successive si fanno a passi
committati, senza mescolarle a cambiamenti di comportamento, con i gate
rieseguiti a ogni passo.

## 7. Distribuzione e piattaforme — il grosso è dopo l'alpha

Fatto: archivio con nome che porta il commit, `MANIFEST.json` con SHA-256 e
byte di ogni file, `.sha256` accanto all'artefatto, rifiuto di sovrascrivere,
workflow che pubblica dal commit esatto del tag; export pubblico verificato
con scansione e controllo dei link.

Restano: firma e notarizzazione degli artefatti; installazione provata su un
host pulito; qualificazione Windows (dialoghi, ACL, Cestino, PTY, rendering,
MCP, processi) e macOS (mai eseguito); la CI di GitHub realmente eseguita sul
commit distribuito.

## 8. Esecuzione, debug e remoto

**Debugger (piano 17, dal 9 novembre 2026):** client DAP `lume-dap` con
`gdb -i dap` reale, pannello Debug con il percorso di esecuzione a cascata
(design TraceLine), breakpoint nel gutter, riga corrente, passi, variabili,
output e valutazione. Widget «Esegui» nella barra del titolo come IntelliJ:
File corrente, configurazioni di `.lume/launch.toml` e goal Maven, con ▶ nel
Terminale e 🐞 con gdb. Provato su Linux con gdb 17.1 e Maven 3.9.16; Windows
e macOS non qualificati, lldb-dap, debugpy e debug Java non ancora provati.

Lo sviluppo remoto via SSH resta confermato nel design e **non
implementato**: nessuna connessione remota.

## Ordine dopo l'alpha

1. Misure e qualifiche mancanti sul binario distribuito (Linux nativo, LSP
   reale, watcher, Jujutsu, processi Git).
2. Tool SQL del server MCP con permessi e consenso, dopo le decisioni su
   ambienti e account di test.
3. Primo client MCP reale collegato all'istanza, in un contesto isolato.
4. Estrazione dei coordinatori da `app.rs`, a passi.
5. Firma degli artefatti e qualificazione Windows; poi macOS.
6. SSH, solo quando esiste un backend da provare; DAP su altre piattaforme e
   altri adattatori dopo una prova reale.

## Criterio di completamento

Nessuna dichiarazione di «completo» finché restano: rischi noti di perdita o
esposizione dei dati, budget non verificati o superati, funzionalità descritte
come presenti ma non collegate all'app, test necessari falliti o rilascio
privo delle verifiche sopra. Ogni incremento riporta separatamente ciò che è
implementato, ciò che è stato verificato e ciò che manca.
