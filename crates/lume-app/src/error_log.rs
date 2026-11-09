//! Log a rotazione degli errori che Lume mostra all'utente.
//!
//! Un errore in status bar sparisce dopo 5 secondi: chi segnala un problema
//! non ha niente da allegare. Qui ogni errore mostrato finisce anche in
//! `<dati>/logs/errors.log`, accanto ai log dei crash.
//!
//! - Tre file, `errors.log` (il corrente), `errors.1.log`, `errors.2.log`, di
//!   al più `TETTO_FILE` byte l'uno: lo spazio occupato ha un massimo noto
//!   (768 KiB).
//! - Si scrive solo quando l'errore accade, nel gestore del messaggio: nessun
//!   timer o thread.
//! - Il testo passa da `crash_log::anonimizza`, perché il file è fatto per
//!   essere allegato a una issue pubblica.
//! - Un errore identico all'ultimo scritto non si riscrive.
//! - Un errore di scrittura non deve mai diventare un secondo errore.

use std::io::Write;
use std::path::PathBuf;

use crate::crash_log::{anonimizza, cartella, cartella_utente};

/// Byte per file. Con tre file lo spazio massimo è 768 KiB.
pub const TETTO_FILE: u64 = 256 * 1024;
const GENERAZIONI: usize = 3;
const CORPO_MAX: usize = 2048;

pub struct ErrorLog {
    dir: PathBuf,
    /// Nome base dei file: `<nome>.log`, `<nome>.1.log`, `<nome>.2.log`.
    nome: &'static str,
    /// Byte massimi del corpo di una riga, oltre si tronca con `…`.
    corpo_max: usize,
    tetto: u64,
    utente: Option<PathBuf>,
    ultimo: Option<String>,
}

impl ErrorLog {
    pub fn new(dir: PathBuf, utente: Option<PathBuf>) -> Self {
        Self {
            dir,
            nome: "errors",
            corpo_max: CORPO_MAX,
            tetto: TETTO_FILE,
            utente,
            ultimo: None,
        }
    }

    /// Restituisce il logger nella cartella dati corrente, se disponibile.
    pub fn predefinito() -> Option<Self> {
        cartella().map(|dir| Self::new(dir, cartella_utente()))
    }

    /// Stessa rotazione e anonimizzazione, su `<nome>.log` e con righe
    /// lunghe fino a `corpo_max` byte. Per il log della grafica.
    pub fn con_nome(mut self, nome: &'static str, corpo_max: usize) -> Self {
        self.nome = nome;
        self.corpo_max = corpo_max;
        self
    }

    #[cfg(test)]
    fn con_tetto(mut self, tetto: u64) -> Self {
        self.tetto = tetto;
        self
    }

    /// Anonimizza, evita duplicati consecutivi e scrive con l'ora locale.
    pub fn registra_ora(&mut self, testo: &str) -> std::io::Result<()> {
        let testo = anonimizza(testo, self.utente.as_deref());
        if self.ultimo.as_deref() == Some(testo.as_str()) {
            return Ok(());
        }
        self.registra(&adesso(), &testo)?;
        self.ultimo = Some(testo);
        Ok(())
    }

    fn registra(&self, quando: &str, testo: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let riga = riga(quando, testo, self.corpo_max);
        let corrente = self.percorso(0);
        let dimensione = std::fs::metadata(&corrente).map_or(0, |meta| meta.len());
        let aggiunta = u64::try_from(riga.len()).unwrap_or(u64::MAX);
        // Un file vuoto accoglie comunque la riga, anche se supera il tetto.
        if dimensione > 0 && dimensione.saturating_add(aggiunta) > self.tetto {
            self.ruota()?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&corrente)?;
        file.write_all(riga.as_bytes())
    }

    fn percorso(&self, generazione: usize) -> PathBuf {
        if generazione == 0 {
            self.dir.join(format!("{}.log", self.nome))
        } else {
            self.dir.join(format!("{}.{generazione}.log", self.nome))
        }
    }

    fn ruota(&self) -> std::io::Result<()> {
        match std::fs::remove_file(self.percorso(GENERAZIONI - 1)) {
            Ok(()) => {}
            Err(errore) if errore.kind() == std::io::ErrorKind::NotFound => {}
            Err(errore) => return Err(errore),
        }
        for da in (0..GENERAZIONI - 1).rev() {
            let origine = self.percorso(da);
            if origine.exists() {
                std::fs::rename(&origine, self.percorso(da + 1))?;
            }
        }
        Ok(())
    }
}

fn adesso() -> String {
    jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S").to_string()
}

fn riga(quando: &str, testo: &str, corpo_max: usize) -> String {
    let mut corpo = testo
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|parte| !parte.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    if corpo.len() > corpo_max {
        let mut fine = corpo_max;
        while !corpo.is_char_boundary(fine) {
            fine -= 1;
        }
        corpo.truncate(fine);
        corpo.push('…');
    }
    format!("{quando} {corpo}\n")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    const QUANDO: &str = "2026-09-29 12:00:00";

    fn log_in(dir: &std::path::Path, tetto: u64) -> ErrorLog {
        ErrorLog::new(dir.to_path_buf(), None).con_tetto(tetto)
    }

    fn leggi(dir: &std::path::Path, nome: &str) -> String {
        std::fs::read_to_string(dir.join(nome)).unwrap_or_default()
    }

    fn registra_da_a(log: &ErrorLog, da: u32, a: u32) {
        for n in da..=a {
            log.registra(QUANDO, &format!("errore {n:02}")).unwrap();
        }
    }

    fn righe(numeri: &[u32]) -> String {
        numeri
            .iter()
            .map(|n| format!("{QUANDO} errore {n:02}\n"))
            .collect()
    }

    #[test]
    fn la_riga_ha_data_e_testo_e_niente_a_capo_dentro() {
        let dir = tempfile::tempdir().unwrap();
        let log = log_in(dir.path(), TETTO_FILE);
        log.registra(QUANDO, "prima\r\nseconda\n\nterza").unwrap();
        assert_eq!(
            leggi(dir.path(), "errors.log"),
            "2026-09-29 12:00:00 prima | seconda | terza\n"
        );
    }

    #[test]
    fn sotto_il_tetto_si_accoda_a_un_solo_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = log_in(dir.path(), 70);
        registra_da_a(&log, 1, 2);
        assert_eq!(leggi(dir.path(), "errors.log"), righe(&[1, 2]));
        assert!(!dir.path().join("errors.1.log").exists());
    }

    #[test]
    fn al_tetto_ruota_e_tiene_tre_generazioni_in_ordine() {
        let dir = tempfile::tempdir().unwrap();
        let log = log_in(dir.path(), 70);
        registra_da_a(&log, 1, 10);
        assert_eq!(leggi(dir.path(), "errors.log"), righe(&[9, 10]));
        assert_eq!(leggi(dir.path(), "errors.1.log"), righe(&[7, 8]));
        assert_eq!(leggi(dir.path(), "errors.2.log"), righe(&[5, 6]));
        assert!(!dir.path().join("errors.3.log").exists());
    }

    #[test]
    fn lo_spazio_occupato_ha_un_massimo_qualunque_sia_il_numero_di_errori() {
        let dir = tempfile::tempdir().unwrap();
        let log = log_in(dir.path(), 70);
        registra_da_a(&log, 1, 99);
        registra_da_a(&log, 1, 99);
        let mut totale = 0;
        let mut file = 0;
        for voce in std::fs::read_dir(dir.path()).unwrap() {
            let voce = voce.unwrap();
            file += 1;
            totale += voce.metadata().unwrap().len();
        }
        assert_eq!(file, 3);
        assert!(totale <= 3 * 70, "{totale} byte su disco");
    }

    #[test]
    fn una_riga_piu_lunga_del_tetto_si_scrive_da_sola_e_la_successiva_ruota() {
        let dir = tempfile::tempdir().unwrap();
        let log = log_in(dir.path(), 70);
        let lunga = "x".repeat(100);
        log.registra(QUANDO, &lunga).unwrap();
        assert_eq!(
            leggi(dir.path(), "errors.log"),
            format!("{QUANDO} {lunga}\n")
        );
        assert!(!dir.path().join("errors.1.log").exists());
        log.registra(QUANDO, "errore 02").unwrap();
        assert_eq!(
            leggi(dir.path(), "errors.1.log"),
            format!("{QUANDO} {lunga}\n")
        );
        assert_eq!(leggi(dir.path(), "errors.log"), righe(&[2]));
    }

    #[test]
    fn un_testo_enorme_si_tronca_senza_spezzare_un_carattere() {
        let dir = tempfile::tempdir().unwrap();
        let log = log_in(dir.path(), TETTO_FILE);
        log.registra(QUANDO, &"€".repeat(2000)).unwrap();
        let byte = std::fs::read(dir.path().join("errors.log")).unwrap();
        let testo = String::from_utf8(byte).expect("UTF-8 valido");
        assert!(testo.ends_with("…\n"), "{}", testo.len());
        assert!(testo.len() <= 19 + 1 + 2048 + 3 + 1, "{}", testo.len());
        assert_eq!(testo.lines().count(), 1);
    }

    #[test]
    fn la_cartella_dei_log_si_crea_se_manca() {
        let dir = tempfile::tempdir().unwrap();
        let profonda = dir.path().join("dati").join("logs");
        let log = log_in(&profonda, TETTO_FILE);
        log.registra(QUANDO, "errore 01").unwrap();
        assert_eq!(leggi(&profonda, "errors.log"), righe(&[1]));
    }

    #[test]
    fn se_la_cartella_non_si_puo_creare_l_errore_torna_al_chiamante() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("occupato");
        std::fs::write(&file, "non sono una cartella").unwrap();
        let log = log_in(&file.join("logs"), TETTO_FILE);
        assert!(log.registra(QUANDO, "errore 01").is_err());
    }

    #[test]
    fn la_cartella_utente_diventa_una_tilde_nel_file() {
        let dir = tempfile::tempdir().unwrap();
        let casa = std::path::PathBuf::from(r"C:\Users\Mario");
        let mut log = ErrorLog::new(dir.path().to_path_buf(), Some(casa));
        log.registra_ora("Apertura di C:\\Users\\Mario\\a.txt fallita: accesso negato")
            .unwrap();
        let testo = leggi(dir.path(), "errors.log");
        assert!(
            testo.contains("Apertura di ~\\a.txt fallita: accesso negato"),
            "{testo}"
        );
        assert!(!testo.contains("Mario"), "{testo}");
    }

    #[test]
    fn lo_stesso_errore_di_fila_si_scrive_una_volta_sola() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = ErrorLog::new(dir.path().to_path_buf(), None);
        for testo in ["x", "x", "x", "y", "x"] {
            log.registra_ora(testo).unwrap();
        }
        let scritto = leggi(dir.path(), "errors.log");
        let corpi: Vec<&str> = scritto.lines().filter_map(|riga| riga.get(20..)).collect();
        assert_eq!(corpi, ["x", "y", "x"]);
    }
}
