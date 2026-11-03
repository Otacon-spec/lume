# Lume

Editor di testo e di codice per un solo utente: questo glossario fissa i nomi
dei concetti di dominio usati nel codice e nella documentazione.

## Scritture

**Mutazione di file**:
Un'operazione che crea, sostituisce, sposta o elimina un file dell'utente:
salvataggio, Salva con nome, Sostituisci nei file, spostamento nel Cestino,
eliminazione di un appunto, scrittura Git del worktree, rinomina dall'albero.
_Avoid_: scrittura (da sola), job, operazione file

**Scrittura in volo**:
Una mutazione di file avviata e non ancora conclusa con un esito.
_Avoid_: operazione in corso, writer attivo, busy

**Coordinatore delle scritture**:
Ciò che decide se una mutazione di file può partire, quali documenti non si
possono modificare nel frattempo e cosa riparte quando una scrittura in volo
finisce.
_Avoid_: coordinamento dei writer, lock delle scritture

**Salvataggio automatico rimandato**:
Un salvataggio automatico rifiutato perché un'altra scrittura era in volo,
che riparte da solo quando quella finisce.
_Avoid_: autosave in coda

**Chiusura in sospeso**:
La domanda «Salvare le modifiche?» aperta per una scheda o per la finestra,
che resta in attesa di una risposta o della fine delle scritture in volo.
_Avoid_: conferma di chiusura, close pending

**Cestino**:
La destinazione di un elemento eliminato dall'albero del progetto, da cui
l'utente può recuperarlo.
_Avoid_: eliminazione definitiva, cancellazione

## Riquadri

**Riquadro modale**:
Un riquadro che, finché è aperto, impedisce ogni altra azione dell'utente
tranne rispondergli o chiuderlo: Informazioni, chiusura in sospeso,
conferma del Cestino, consenso MCP, scheda MCP, Scorciatoie.
_Avoid_: dialog, popup, overlay (che comprende anche palette e menu)

**Overlay**:
Qualsiasi elemento disegnato sopra l'editor, modale o no: menu, palette,
Trova, Vai a riga, riquadri modali.
_Avoid_: finestra, pannello

## Appunti

**Libreria Appunti**:
La raccolta personale di appunti associata alla cartella scelta dall'utente.
_Avoid_: cartella attiva (senza specificare Appunti), store

**Richiesta di scansione Appunti**:
Una lettura dell'elenco degli appunti della libreria in un determinato momento;
due letture della stessa libreria sono richieste distinte.
_Avoid_: generazione (senza specificare che cosa identifica)

**Origine della vista Appunti**:
La libreria a cui apparteneva il contenuto mostrato quando l'utente ha compiuto
un gesto, distinta dalla libreria mostrata al recapito del gesto.
_Avoid_: cartella corrente (per indicare la provenienza di un gesto passato)

**Intento di creazione di un appunto**:
La richiesta dell'utente di creare un appunto, ancora in attesa che la libreria
sia disponibile.
_Avoid_: creazione in volo (quando nessuna scrittura è ancora iniziata)

## Relationships

- Una **Scrittura in volo** è una **Mutazione di file** ammessa dal
  **Coordinatore delle scritture**.
- Mentre una **Scrittura in volo** non è un salvataggio, ogni altra
  **Mutazione di file** è rifiutata; i salvataggi si accodano solo fra loro.
- Una **Chiusura in sospeso** procede da sola quando non resta alcuna
  **Scrittura in volo**, salvo errori.
- Un **Riquadro modale** è un **Overlay**; un **Riquadro modale** non ne
  sostituisce un altro, tranne la **Chiusura in sospeso**.
