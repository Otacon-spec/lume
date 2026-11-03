# Salvataggi rifiutati durante altre scritture

Mentre è in volo una mutazione di file che non è un salvataggio (Cestino,
Sostituisci nei file, eliminazione di un appunto, scrittura Git, Salva con
nome), un salvataggio richiesto dall'utente, Ctrl+S compreso, è rifiutato
con un avviso in status bar invece di essere accodato; i salvataggi si
accodano solo fra loro, così «Salva tutto» funziona, e un salvataggio
automatico rifiutato è rimandato e riparte da solo a fine operazione. Il
proprietario ha scelto una regola unica e visibile («una scrittura alla
volta, il resto rifiutato con avviso») contro la coda trasversale proposta,
che nascondeva quando sarebbe partita l'azione dell'utente.

## Considered Options

- Coda trasversale: Ctrl+S accodato durante qualsiasi operazione e avviato
  alla fine. Scartata: l'azione parte più tardi senza che l'utente lo veda.
- Tutto in coda, anche Cestino e Sostituisci: scartata, una coda per tipo e
  azioni differite senza controllo dell'utente.
