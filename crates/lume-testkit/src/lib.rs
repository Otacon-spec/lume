//! Strumenti esterni richiesti dai test di Lume (solo dev-dependency).
//!
//! Molti test di integrazione lanciano `git`, `rustc`, una shell con PTY,
//! `tasklist`/`taskkill` o `kill`, oppure rilanciano il proprio eseguibile. Se
//! lo strumento manca, un `unwrap()` dà un panic senza spiegazioni — e in un
//! caso (`process_exists` di `pty_session.rs`) l'errore veniva letto come
//! «processo sparito» e il test **passava per il motivo sbagliato**.
//!
//! Qui la regola è una sola: un test dichiara con `richiedi!` ciò che gli serve.
//! Se manca, **salta** stampando cosa manca e come procurarselo; con
//! `LUME_TEST_RICHIEDI_STRUMENTI=1` (la CI) **fallisce** con lo stesso testo,
//! così un runner senza `git` non produce mai un falso verde.
//!
//! La ricerca non lancia processi: scorre `PATH` e controlla che il file esista.

use std::fmt;
use std::path::PathBuf;

/// Nome della variabile d'ambiente che trasforma il salto in un fallimento.
pub const VARIABILE_RIGIDA: &str = "LUME_TEST_RICHIEDI_STRUMENTI";

/// Uno strumento esterno che un test può richiedere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strumento {
    Git,
    Jujutsu,
    Rustc,
    /// `cmd.exe` su Windows, `sh` altrove: la shell che il PTY di prova lancia.
    ShellPty,
    /// Chi dice se un pid esiste: `tasklist` su Windows, `kill -0` altrove.
    ElencoProcessi,
    /// Chi termina un pid: `taskkill` su Windows, `kill -9` altrove.
    Terminatore,
    /// L'eseguibile di test in corso (`std::env::current_exe`): i test che
    /// lanciano un sottoprocesso di se stessi.
    EseguibileCorrente,
    /// GNU gdb, che dalla 14 parla il Debug Adapter Protocol (`gdb -i dap`).
    Gdb,
    /// Il compilatore C di sistema (`cc`): compila i programmi da debuggare.
    CompilatoreC,
}

impl Strumento {
    /// Nome corto, quello che si scrive nel messaggio.
    pub const fn nome(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Jujutsu => "jj",
            Self::Rustc => "rustc",
            Self::ShellPty => {
                if cfg!(windows) {
                    "cmd"
                } else {
                    "sh"
                }
            }
            Self::ElencoProcessi => {
                if cfg!(windows) {
                    "tasklist"
                } else {
                    "kill"
                }
            }
            Self::Terminatore => {
                if cfg!(windows) {
                    "taskkill"
                } else {
                    "kill"
                }
            }
            Self::EseguibileCorrente => "eseguibile-corrente",
            Self::Gdb => "gdb",
            Self::CompilatoreC => "cc",
        }
    }

    /// Come procurarselo, in una frase.
    pub const fn come_procurarselo(self) -> &'static str {
        match self {
            Self::Git => "installa Git (git-scm.com) e mettilo su PATH",
            Self::Jujutsu => "installa Jujutsu (jj-vcs.github.io) e mettilo su PATH",
            Self::Rustc => "installa la toolchain Rust (rustup.rs) e mettila su PATH",
            Self::ShellPty => {
                if cfg!(windows) {
                    "cmd.exe sta in C:\\Windows\\System32: controlla PATH"
                } else {
                    "installa una shell POSIX come sh e mettila su PATH"
                }
            }
            Self::ElencoProcessi => {
                if cfg!(windows) {
                    "tasklist.exe sta in C:\\Windows\\System32: controlla PATH"
                } else {
                    "installa procps o util-linux, che forniscono kill, e mettili su PATH"
                }
            }
            Self::Terminatore => {
                if cfg!(windows) {
                    "taskkill.exe sta in C:\\Windows\\System32: controlla PATH"
                } else {
                    "installa procps o util-linux, che forniscono kill, e mettili su PATH"
                }
            }
            Self::EseguibileCorrente => {
                "il binario di test non esiste più su disco: rilancia cargo test"
            }
            Self::Gdb => "installa GNU gdb 14 o successivo (con Python) e mettilo su PATH",
            Self::CompilatoreC => {
                "installa un compilatore C (gcc o clang, che forniscono cc) e mettilo su PATH"
            }
        }
    }

    /// Nome del file eseguibile da cercare in ogni cartella di `PATH`.
    fn file(self) -> String {
        format!("{}{}", self.nome(), if cfg!(windows) { ".exe" } else { "" })
    }
}

/// Uno strumento assente, con il messaggio per chi legge il log del test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mancante(pub Strumento);

impl fmt::Display for Mancante {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "manca `{}` ({})",
            self.0.nome(),
            self.0.come_procurarselo()
        )
    }
}

/// Cerca `file` in `cartelle`. Le voci vuote o relative si scartano: una voce
/// vuota di `PATH` vuol dire «cartella corrente» e farebbe trovare uno
/// strumento messo lì da chiunque (stessa regola di `discovery.rs`).
fn cerca(file: &str, cartelle: &[PathBuf]) -> Option<PathBuf> {
    cartelle
        .iter()
        .filter(|cartella| cartella.is_absolute())
        .map(|cartella| cartella.join(file))
        .find(|candidato| candidato.is_file())
}

/// Come [`trova`], ma su un elenco di cartelle dato (per i test).
pub fn trova_in(strumento: Strumento, cartelle: &[PathBuf]) -> Option<PathBuf> {
    if strumento == Strumento::EseguibileCorrente {
        return std::env::current_exe()
            .ok()
            .filter(|percorso| percorso.is_file());
    }
    cerca(&strumento.file(), cartelle)
}

/// Il file dello strumento, se sta in una cartella assoluta di `PATH`.
pub fn trova(strumento: Strumento) -> Option<PathBuf> {
    let cartelle: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|valore| std::env::split_paths(&valore).collect())
        .unwrap_or_default();
    trova_in(strumento, &cartelle)
}

/// Cosa fare con un elenco di strumenti mancanti.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decisione {
    Procedi,
    Salta(String),
    Fallisci(String),
}

/// Funzione pura: dato ciò che manca e la modalità, decide. Testabile senza
/// toccare l'ambiente.
pub fn decidi(mancanti: &[Strumento], rigida: bool) -> Decisione {
    if mancanti.is_empty() {
        return Decisione::Procedi;
    }
    let elenco = mancanti
        .iter()
        .map(|strumento| Mancante(*strumento).to_string())
        .collect::<Vec<_>>()
        .join("; ");
    if rigida {
        Decisione::Fallisci(format!(
            "{VARIABILE_RIGIDA}=1: strumenti richiesti assenti: {elenco}"
        ))
    } else {
        Decisione::Salta(format!(
            "SALTATO: {elenco}. Con {VARIABILE_RIGIDA}=1 questo test fallisce invece di saltare."
        ))
    }
}

fn rigida_da(valore: Option<&str>) -> bool {
    valore == Some("1")
}

/// `true` se la variabile d'ambiente chiede il fallimento invece del salto.
pub fn modalita_rigida() -> bool {
    rigida_da(std::env::var(VARIABILE_RIGIDA).ok().as_deref())
}

/// Controlla `strumenti` con la modalità corrente.
pub fn verifica(strumenti: &[Strumento]) -> Decisione {
    let mancanti: Vec<Strumento> = strumenti
        .iter()
        .copied()
        .filter(|strumento| trova(*strumento).is_none())
        .collect();
    decidi(&mancanti, modalita_rigida())
}

/// `true` se il test può procedere. Se manca qualcosa stampa il motivo su
/// stderr e restituisce `false` (il test deve uscire); in modalità rigida
/// va in panic con lo stesso testo.
///
/// Il nome del test viene dal thread corrente; nei binari con `harness = false`
/// è `main`.
pub fn richiedi(strumenti: &[Strumento]) -> bool {
    match verifica(strumenti) {
        Decisione::Procedi => true,
        Decisione::Salta(messaggio) => {
            let nome = std::thread::current()
                .name()
                .map_or_else(|| "test".to_owned(), str::to_owned);
            eprintln!("{nome}: {messaggio}");
            false
        }
        Decisione::Fallisci(messaggio) => panic!("{messaggio}"),
    }
}

/// Prima istruzione di un test che dipende da strumenti esterni:
/// `lume_testkit::richiedi!(Strumento::Git);`
#[macro_export]
macro_rules! richiedi {
    ($($strumento:expr),+ $(,)?) => {
        if !$crate::richiedi(&[$($strumento),+]) {
            return;
        }
    };
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn senza_mancanti_si_procede() {
        assert_eq!(decidi(&[], false), Decisione::Procedi);
        assert_eq!(decidi(&[], true), Decisione::Procedi);
    }

    #[test]
    fn se_manca_qualcosa_si_salta_e_si_dice_cosa() {
        let Decisione::Salta(messaggio) = decidi(&[Strumento::Git, Strumento::Rustc], false) else {
            panic!("doveva saltare");
        };
        assert!(messaggio.starts_with("SALTATO"), "{messaggio}");
        assert!(messaggio.contains("`git`"), "{messaggio}");
        assert!(messaggio.contains("`rustc`"), "{messaggio}");
        assert!(messaggio.contains(VARIABILE_RIGIDA), "{messaggio}");
    }

    #[test]
    fn in_modalita_rigida_si_fallisce_con_lo_stesso_elenco() {
        let Decisione::Fallisci(messaggio) = decidi(&[Strumento::Git], true) else {
            panic!("doveva fallire");
        };
        assert!(messaggio.contains("`git`"), "{messaggio}");
        assert!(messaggio.contains(VARIABILE_RIGIDA), "{messaggio}");
    }

    #[test]
    fn il_messaggio_dice_come_procurarsi_lo_strumento() {
        for strumento in [
            Strumento::Git,
            Strumento::Rustc,
            Strumento::ShellPty,
            Strumento::ElencoProcessi,
            Strumento::Terminatore,
            Strumento::EseguibileCorrente,
            Strumento::Gdb,
            Strumento::CompilatoreC,
        ] {
            let testo = Mancante(strumento).to_string();
            assert!(testo.contains(strumento.nome()), "{testo}");
            assert!(strumento.come_procurarselo().len() > 10, "{testo}");
        }
    }

    #[test]
    fn le_voci_vuote_o_relative_di_path_si_scartano() {
        // Il test gira con la cartella del crate come cartella corrente, che
        // contiene `Cargo.toml`: una voce vuota o relativa lo troverebbe.
        assert!(cerca("Cargo.toml", &[PathBuf::new()]).is_none());
        assert!(cerca("Cargo.toml", &[PathBuf::from(".")]).is_none());
        assert!(cerca("Cargo.toml", &[PathBuf::from("..").join("lume-testkit")]).is_none());
        // La stessa cartella, data in modo assoluto, lo trova.
        let assoluta = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        assert!(cerca("Cargo.toml", &[assoluta]).is_some());
    }

    #[test]
    fn percorsi_con_spazi_e_accenti() {
        let cartella = tempfile::tempdir().unwrap();
        let strana = cartella.path().join("cartella con spazi e àèì");
        std::fs::create_dir_all(&strana).unwrap();
        std::fs::write(strana.join("strumento di prova.exe"), b"").unwrap();
        let trovato = cerca("strumento di prova.exe", std::slice::from_ref(&strana)).unwrap();
        assert_eq!(trovato, strana.join("strumento di prova.exe"));
    }

    #[test]
    fn una_directory_non_e_uno_strumento() {
        let cartella = tempfile::tempdir().unwrap();
        std::fs::create_dir(cartella.path().join("git.exe")).unwrap();
        std::fs::create_dir(cartella.path().join("git")).unwrap();
        assert!(trova_in(Strumento::Git, &[cartella.path().to_path_buf()]).is_none());
    }

    #[test]
    fn trova_in_usa_il_nome_giusto_per_la_piattaforma() {
        let cartella = tempfile::tempdir().unwrap();
        let nome = if cfg!(windows) { "git.exe" } else { "git" };
        std::fs::write(cartella.path().join(nome), b"").unwrap();
        let trovato = trova_in(Strumento::Git, &[cartella.path().to_path_buf()]).unwrap();
        assert!(trovato.ends_with(nome));
    }

    #[test]
    fn l_eseguibile_corrente_esiste_sempre() {
        assert!(trova(Strumento::EseguibileCorrente).is_some());
        assert!(trova_in(Strumento::EseguibileCorrente, &[]).is_some());
    }

    #[test]
    fn la_variabile_rigida_vale_solo_uno() {
        // Non tocca l'ambiente del processo (i test girano in parallelo):
        // prova la funzione pura che la interpreta.
        assert!(rigida_da(Some("1")));
        assert!(!rigida_da(Some("0")));
        assert!(!rigida_da(Some("")));
        assert!(!rigida_da(Some("true")));
        assert!(!rigida_da(None));
    }
}
