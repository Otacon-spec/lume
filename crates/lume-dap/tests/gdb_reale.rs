//! Prova con un adattatore vero: `gdb -i dap` su un programma C compilato
//! qui. Senza gdb o cc salta; con `LUME_TEST_RICHIEDI_STRUMENTI=1` fallisce.
//!
//! Solo Linux: è l'unica piattaforma su cui il debugger è provato (piano 17).
//! Su Windows e macOS la CI non ha gdb con DAP, e il test fallirebbe in
//! modalità rigida senza dire niente del prodotto.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use lume_dap::{
    AdapterEvent, AdapterId, DebugSession, Event, FileBreakpoints, LaunchConfig, OutputEvent,
    ShutdownOutcome, SourceBreakpoint, StopHistory, StoppedEvent,
};
use lume_testkit::Strumento;
use tokio::sync::mpsc;

const PROGRAMMA: &str = r#"#include <stdio.h>

static int quadrato(int x) {
    int r = x * x;
    return r;
}

static int somma(int n) {
    int totale = 0;
    for (int i = 1; i <= n; i++) {
        totale += quadrato(i);
    }
    return totale;
}

int main(void) {
    int n = 3;
    int risultato = somma(n);
    printf("risultato=%d\n", risultato);
    return 0;
}
"#;

const ATTESA: Duration = Duration::from_secs(30);

fn compila(cartella: &Path) -> std::path::PathBuf {
    let sorgente = cartella.join("prova.c");
    std::fs::write(&sorgente, PROGRAMMA).unwrap();
    let eseguibile = cartella.join("prova");
    let esito = std::process::Command::new(lume_testkit::trova(Strumento::CompilatoreC).unwrap())
        .args(["-g", "-O0", "-o"])
        .arg(&eseguibile)
        .arg(&sorgente)
        .status()
        .unwrap();
    assert!(esito.success(), "cc fallito: {esito}");
    eseguibile
}

/// Il prossimo evento `nome`, raccogliendo l'output nel frattempo.
async fn attendi(
    eventi: &mut mpsc::Receiver<AdapterEvent>,
    nome: &str,
    output: &mut String,
) -> Event {
    let ricerca = async {
        loop {
            match eventi.recv().await {
                Some(AdapterEvent::Event(evento)) => {
                    if evento.event == "output" {
                        let corpo: OutputEvent =
                            serde_json::from_value(evento.body.clone()).unwrap();
                        output.push_str(&corpo.output);
                    }
                    if evento.event == nome {
                        return evento;
                    }
                }
                Some(AdapterEvent::Closed(motivo)) => panic!("chiuso prima di `{nome}`: {motivo}"),
                None => panic!("eventi finiti prima di `{nome}`"),
            }
        }
    };
    tokio::time::timeout(ATTESA, ricerca)
        .await
        .unwrap_or_else(|_| panic!("nessun evento `{nome}` entro {ATTESA:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gdb_reale_breakpoint_passi_variabili_e_uscita() {
    lume_testkit::richiedi!(Strumento::Gdb, Strumento::CompilatoreC);
    let cartella = tempfile::tempdir().unwrap();
    let eseguibile = compila(cartella.path());
    let sorgente = cartella.path().join("prova.c");
    let config = LaunchConfig {
        name: "prova".into(),
        adapter: AdapterId::Gdb,
        program: eseguibile,
        args: Vec::new(),
        cwd: cartella.path().to_path_buf(),
        env: BTreeMap::new(),
        stop_at_entry: false,
    };
    let gdb = AdapterId::Gdb.find().expect("gdb su PATH");
    let punti = FileBreakpoints {
        path: sorgente.clone(),
        breakpoints: vec![SourceBreakpoint {
            line: 4,
            condition: None,
        }],
    };
    let avviata = DebugSession::start(&gdb, &config, std::slice::from_ref(&punti))
        .await
        .unwrap_or_else(|errore| panic!("avvio: {errore}"));
    let sessione = avviata.session;
    let mut eventi = avviata.events;
    let mut output = String::new();
    assert!(sessione.capabilities().supports_conditional_breakpoints);
    assert_eq!(avviata.breakpoints.len(), 1);

    // Primo arresto: breakpoint in quadrato(1).
    let fermo = attendi(&mut eventi, "stopped", &mut output).await;
    let fermo: StoppedEvent = serde_json::from_value(fermo.body).unwrap();
    assert_eq!(fermo.reason, "breakpoint");
    let thread = fermo.thread_id.unwrap();
    let frames = sessione.stack_trace(thread).await.unwrap();
    let nomi: Vec<&str> = frames.iter().take(3).map(|f| f.name.as_str()).collect();
    assert_eq!(nomi, ["quadrato", "somma", "main"]);
    assert_eq!(frames[0].line, 4);
    let mut storia = StopHistory::new();
    storia.push(fermo.reason.clone(), None, thread, &frames);

    let scope = sessione.scopes(frames[0].id).await.unwrap();
    let argomenti = scope
        .iter()
        .find(|s| s.variables_reference > 0)
        .expect("uno scope con variabili");
    let (variabili, troncate) = sessione
        .variables(argomenti.variables_reference)
        .await
        .unwrap();
    assert!(!troncate);
    assert!(
        variabili.iter().any(|v| v.name == "x" && v.value == "1"),
        "{variabili:?}"
    );

    // Passo sopra: riga 5.
    sessione.next(thread).await.unwrap();
    let fermo: StoppedEvent =
        serde_json::from_value(attendi(&mut eventi, "stopped", &mut output).await.body).unwrap();
    let frames = sessione.stack_trace(thread).await.unwrap();
    assert_eq!(frames[0].line, 5);
    storia.push(fermo.reason, None, thread, &frames);

    // Esci: torna in somma.
    sessione.step_out(thread).await.unwrap();
    let fermo: StoppedEvent =
        serde_json::from_value(attendi(&mut eventi, "stopped", &mut output).await.body).unwrap();
    let frames = sessione.stack_trace(thread).await.unwrap();
    assert_eq!(frames[0].name, "somma");
    storia.push(fermo.reason, None, thread, &frames);
    // In gdb `n` come comando sarebbe `next`: il contesto watch lo valuta
    // come variabile e il programma non avanza.
    let valore = sessione.evaluate("n", Some(frames[0].id)).await.unwrap();
    assert_eq!(valore.result, "3");
    let ancora = sessione.stack_trace(thread).await.unwrap();
    assert_eq!(ancora[0].line, frames[0].line);

    // Continua: di nuovo il breakpoint, con x = 2.
    sessione.continue_(thread).await.unwrap();
    attendi(&mut eventi, "stopped", &mut output).await;
    let frames = sessione.stack_trace(thread).await.unwrap();
    assert_eq!(frames[0].line, 4);
    let valore = sessione.evaluate("x", Some(frames[0].id)).await.unwrap();
    assert_eq!(valore.result, "2");
    storia.push("breakpoint".into(), None, thread, &frames);
    assert_eq!(storia.position(), Some((4, 4)));
    assert_eq!(
        storia.lines_hit(&sorgente).into_iter().collect::<Vec<_>>(),
        [4, 5, 11]
    );

    // Tolti i breakpoint, il programma arriva in fondo.
    let senza = FileBreakpoints {
        path: sorgente,
        breakpoints: Vec::new(),
    };
    assert!(sessione.set_breakpoints(&senza).await.unwrap().is_empty());
    sessione.continue_(thread).await.unwrap();
    let uscito = attendi(&mut eventi, "exited", &mut output).await;
    assert_eq!(uscito.body["exitCode"], 0);
    attendi(&mut eventi, "terminated", &mut output).await;
    assert!(output.contains("risultato=14"), "{output}");

    let esito = sessione.close(true).await;
    assert!(matches!(esito, ShutdownOutcome::Exited(_)), "{esito:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gdb_reale_programma_inesistente_e_errore_esplicito() {
    lume_testkit::richiedi!(Strumento::Gdb);
    let cartella = tempfile::tempdir().unwrap();
    let config = LaunchConfig {
        name: "assente".into(),
        adapter: AdapterId::Gdb,
        program: cartella.path().join("non-esiste"),
        args: Vec::new(),
        cwd: cartella.path().to_path_buf(),
        env: BTreeMap::new(),
        stop_at_entry: false,
    };
    let gdb = AdapterId::Gdb.find().expect("gdb su PATH");
    let Err(errore) = DebugSession::start(&gdb, &config, &[]).await else {
        panic!("l'avvio di un programma inesistente deve fallire");
    };
    assert!(errore.message.contains("non-esiste"), "{}", errore.message);
}
