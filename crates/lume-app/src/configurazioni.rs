//! Le configurazioni di esecuzione del progetto, in `.lume/launch.toml`
//! (piano 17, finestra «Configurazioni di esecuzione» stile IntelliJ).
//!
//! Tre tipi:
//! - `applicazione`: un programma, eseguibile con ▶ e debuggabile con gdb;
//! - `maven`: goal e profili Maven, nella cartella del progetto;
//! - `comando`: un programma qualsiasi, solo ▶.
//!
//! Il file è l'unica fonte: la finestra lo legge e lo riscrive, e si può
//! sempre modificare a mano. Leggere non esegue niente.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lume_dap::{AdapterId, LaunchConfig, LaunchError};
use serde::{Deserialize, Serialize};

pub use lume_dap::launch::{LAUNCH_FILE, MAX_LAUNCH_FILE_BYTES};

const WORKSPACE_FOLDER: &str = "${workspaceFolder}";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tipo {
    #[default]
    Applicazione,
    Maven,
    Comando,
}

impl Tipo {
    pub const ALL: [Tipo; 3] = [Tipo::Applicazione, Tipo::Maven, Tipo::Comando];

    pub const fn etichetta(self) -> &'static str {
        match self {
            Tipo::Applicazione => "Applicazione",
            Tipo::Maven => "Maven",
            Tipo::Comando => "Comando",
        }
    }
}

impl std::fmt::Display for Tipo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.etichetta())
    }
}

fn is_false(valore: &bool) -> bool {
    !*valore
}

/// Una configurazione come sta nel file: testi non sostituiti, così la
/// finestra la riscrive uguale.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configurazione {
    pub nome: String,
    #[serde(default)]
    pub tipo: Tipo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adattatore: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub richiesta: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub programma: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argomenti: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cartella: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ambiente: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub goal: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profili: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub fermati_all_inizio: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    configurazione: Vec<Configurazione>,
}

/// Interpreta il testo del file.
pub fn leggi_testo(testo: &str) -> Result<Vec<Configurazione>, LaunchError> {
    let file: File =
        toml::from_str(testo).map_err(|errore| LaunchError::Syntax(errore.to_string()))?;
    Ok(file.configurazione)
}

/// Legge `.lume/launch.toml` (I/O bloccante: fuori dalla UI). Un file
/// assente è un elenco vuoto, non un errore.
pub fn leggi(radice: &Path) -> Result<Vec<Configurazione>, LaunchError> {
    let percorso = radice.join(LAUNCH_FILE);
    let metadati = match std::fs::metadata(&percorso) {
        Ok(metadati) => metadati,
        Err(errore) if errore.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(errore) => return Err(LaunchError::Unreadable(errore.to_string())),
    };
    if metadati.len() > MAX_LAUNCH_FILE_BYTES {
        return Err(LaunchError::TooLarge);
    }
    let testo = std::fs::read_to_string(&percorso)
        .map_err(|errore| LaunchError::Unreadable(errore.to_string()))?;
    leggi_testo(&testo)
}

/// Il testo del file per queste configurazioni.
pub fn scrivi_testo(configurazioni: &[Configurazione]) -> Result<String, String> {
    let corpo = toml::to_string(&File {
        configurazione: configurazioni.to_vec(),
    })
    .map_err(|errore| errore.to_string())?;
    Ok(format!(
        "# Configurazioni di esecuzione di Lume: le scrive la finestra\n\
         # «Configurazioni di esecuzione», ma si possono modificare a mano.\n\n{corpo}"
    ))
}

/// Scrive il file sostituendolo per intero (file temporaneo e rinomina).
pub fn scrivi(radice: &Path, configurazioni: &[Configurazione]) -> Result<(), String> {
    let testo = scrivi_testo(configurazioni)?;
    let cartella = radice.join(".lume");
    std::fs::create_dir_all(&cartella).map_err(|errore| errore.to_string())?;
    let temporaneo = cartella.join(format!("launch.toml.{}.tmp", std::process::id()));
    std::fs::write(&temporaneo, testo).map_err(|errore| errore.to_string())?;
    std::fs::rename(&temporaneo, radice.join(LAUNCH_FILE)).map_err(|errore| {
        let _ = std::fs::remove_file(&temporaneo);
        errore.to_string()
    })
}

fn sostituisci(valore: &str, radice: &Path) -> String {
    valore.replace(WORKSPACE_FOLDER, &radice.to_string_lossy())
}

fn percorso_assoluto(valore: &str, radice: &Path, campo: &str) -> Result<PathBuf, String> {
    let percorso = PathBuf::from(sostituisci(valore, radice));
    if percorso.is_absolute() {
        Ok(percorso)
    } else {
        Err(format!(
            "`{campo}` deve essere un percorso assoluto (anche con {WORKSPACE_FOLDER})"
        ))
    }
}

/// Quel che serve per eseguire una configurazione, sostituzioni fatte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Risolta {
    pub programma: PathBuf,
    pub argomenti: Vec<String>,
    pub cartella: PathBuf,
    pub ambiente: Vec<(String, String)>,
}

impl Configurazione {
    pub fn nuova(tipo: Tipo, nome: String) -> Self {
        match tipo {
            Tipo::Applicazione => Self {
                nome,
                tipo,
                adattatore: Some(AdapterId::Gdb.key().to_owned()),
                programma: format!("{WORKSPACE_FOLDER}/programma"),
                ..Self::default()
            },
            Tipo::Maven => Self {
                nome,
                tipo,
                goal: vec!["package".into()],
                ..Self::default()
            },
            Tipo::Comando => Self {
                nome,
                tipo,
                ..Self::default()
            },
        }
    }

    fn cartella(&self, radice: &Path) -> Result<PathBuf, String> {
        self.cartella.as_deref().map_or_else(
            || Ok(radice.to_path_buf()),
            |cartella| percorso_assoluto(cartella, radice, "cartella"),
        )
    }

    fn ambiente(&self, radice: &Path) -> Vec<(String, String)> {
        self.ambiente
            .iter()
            .map(|(chiave, valore)| (chiave.clone(), sostituisci(valore, radice)))
            .collect()
    }

    /// Che cosa manca perché la configurazione sia valida.
    pub fn problema(&self) -> Option<String> {
        if self.nome.trim().is_empty() {
            return Some("il nome è vuoto".into());
        }
        match self.tipo {
            Tipo::Applicazione | Tipo::Comando if self.programma.trim().is_empty() => {
                Some("manca il programma".into())
            }
            Tipo::Maven if self.goal.is_empty() => Some("manca almeno un goal".into()),
            Tipo::Applicazione
                if self
                    .adattatore
                    .as_deref()
                    .is_some_and(|chiave| AdapterId::from_key(chiave).is_none()) =>
            {
                Some("adattatore sconosciuto (noto: gdb)".into())
            }
            _ if self.richiesta.as_deref().is_some_and(|r| r != "launch") => {
                Some("la v1 avvia soltanto (`richiesta = \"launch\"`)".into())
            }
            _ => None,
        }
    }

    /// Programma e argomenti per ▶. Per Maven `comando_maven` è il
    /// `mvnw`/`mvn` trovato.
    pub fn risolvi(&self, radice: &Path, comando_maven: Option<&Path>) -> Result<Risolta, String> {
        if let Some(problema) = self.problema() {
            return Err(problema);
        }
        let cartella = self.cartella(radice)?;
        match self.tipo {
            Tipo::Applicazione | Tipo::Comando => {
                let programma = sostituisci(&self.programma, radice);
                let con_percorso = programma.contains('/') || programma.contains('\\');
                let programma = if self.tipo == Tipo::Comando && !con_percorso {
                    // Un comando senza percorso si cerca su PATH, come in un terminale.
                    PathBuf::from(programma)
                } else {
                    percorso_assoluto(&self.programma, radice, "programma")?
                };
                Ok(Risolta {
                    programma,
                    argomenti: self
                        .argomenti
                        .iter()
                        .map(|argomento| sostituisci(argomento, radice))
                        .collect(),
                    cartella,
                    ambiente: self.ambiente(radice),
                })
            }
            Tipo::Maven => {
                let programma = comando_maven
                    .map(Path::to_path_buf)
                    .ok_or_else(|| "né mvnw nel progetto né mvn su PATH".to_owned())?;
                let mut argomenti = vec!["-B".to_owned()];
                if !self.profili.is_empty() {
                    argomenti.push(format!("-P{}", self.profili.join(",")));
                }
                argomenti.extend(self.argomenti.iter().map(|a| sostituisci(a, radice)));
                argomenti.extend(self.goal.iter().cloned());
                Ok(Risolta {
                    programma,
                    argomenti,
                    cartella,
                    ambiente: self.ambiente(radice),
                })
            }
        }
    }

    /// La configurazione di debug, solo per il tipo applicazione.
    pub fn lancio(&self, radice: &Path) -> Option<Result<LaunchConfig, String>> {
        if self.tipo != Tipo::Applicazione {
            return None;
        }
        let adattatore = self.adattatore.as_deref().unwrap_or(AdapterId::Gdb.key());
        Some(self.risolvi(radice, None).and_then(|risolta| {
            Ok(LaunchConfig {
                name: self.nome.trim().to_owned(),
                adapter: AdapterId::from_key(adattatore)
                    .ok_or_else(|| "adattatore sconosciuto".to_owned())?,
                program: risolta.programma,
                args: risolta.argomenti,
                cwd: risolta.cartella,
                env: risolta.ambiente.into_iter().collect(),
                stop_at_entry: self.fermati_all_inizio,
            })
        }))
    }
}

/// Le configurazioni di debug, nell'ordine del file; la prima non valida è
/// un errore che dice quale.
pub fn lanci(
    configurazioni: &[Configurazione],
    radice: &Path,
) -> Result<Vec<LaunchConfig>, LaunchError> {
    configurazioni
        .iter()
        .enumerate()
        .filter_map(|(indice, configurazione)| {
            configurazione.lancio(radice).map(|esito| {
                esito.map_err(|detail| LaunchError::Invalid {
                    index: indice + 1,
                    detail,
                })
            })
        })
        .collect()
}

// ── Testi dei campi della finestra ──────────────────────────────────

/// Divide una riga di argomenti: spazi come separatori, virgolette doppie
/// per tenere insieme, `\` per un carattere letterale. Niente shell.
pub fn dividi_argomenti(riga: &str) -> Vec<String> {
    let mut argomenti = Vec::new();
    let mut corrente = String::new();
    let mut dentro = false;
    let mut iniziato = false;
    let mut caratteri = riga.chars();
    while let Some(carattere) = caratteri.next() {
        match carattere {
            '\\' => {
                if let Some(prossimo) = caratteri.next() {
                    corrente.push(prossimo);
                    iniziato = true;
                }
            }
            '"' => {
                dentro = !dentro;
                iniziato = true;
            }
            c if c.is_whitespace() && !dentro => {
                if iniziato {
                    argomenti.push(std::mem::take(&mut corrente));
                    iniziato = false;
                }
            }
            c => {
                corrente.push(c);
                iniziato = true;
            }
        }
    }
    if iniziato {
        argomenti.push(corrente);
    }
    argomenti
}

/// L'inverso di [`dividi_argomenti`].
pub fn unisci_argomenti(argomenti: &[String]) -> String {
    argomenti
        .iter()
        .map(|argomento| {
            if argomento.is_empty()
                || argomento
                    .chars()
                    .any(|c| c.is_whitespace() || c == '"' || c == '\\')
            {
                format!(
                    "\"{}\"",
                    argomento.replace('\\', "\\\\").replace('"', "\\\"")
                )
            } else {
                argomento.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `CHIAVE=valore` separati da `;`.
pub fn dividi_ambiente(testo: &str) -> Result<BTreeMap<String, String>, String> {
    testo
        .split(';')
        .map(str::trim)
        .filter(|voce| !voce.is_empty())
        .map(|voce| {
            let (chiave, valore) = voce
                .split_once('=')
                .ok_or_else(|| format!("«{voce}» non è CHIAVE=valore"))?;
            let chiave = chiave.trim();
            if chiave.is_empty() {
                return Err(format!("«{voce}» ha la chiave vuota"));
            }
            Ok((chiave.to_owned(), valore.trim().to_owned()))
        })
        .collect()
}

pub fn unisci_ambiente(ambiente: &BTreeMap<String, String>) -> String {
    ambiente
        .iter()
        .map(|(chiave, valore)| format!("{chiave}={valore}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Elenco separato da virgole o spazi (profili, goal).
pub fn dividi_elenco(testo: &str) -> Vec<String> {
    testo
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|voce| !voce.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn i_tre_tipi_fanno_andata_e_ritorno_dal_file() {
        let configurazioni = vec![
            Configurazione::nuova(Tipo::Applicazione, "app".into()),
            Configurazione {
                profili: vec!["dev".into()],
                ..Configurazione::nuova(Tipo::Maven, "pacchetto".into())
            },
            Configurazione {
                programma: "python3".into(),
                argomenti: vec!["script.py".into(), "con spazio".into()],
                ..Configurazione::nuova(Tipo::Comando, "script".into())
            },
        ];
        let testo = scrivi_testo(&configurazioni).unwrap();
        assert_eq!(leggi_testo(&testo).unwrap(), configurazioni);
    }

    /// Una radice assoluta su ogni piattaforma (la CI gira anche su Windows).
    fn radice() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from("C:\\r")
        } else {
            PathBuf::from("/r")
        }
    }

    #[test]
    fn il_vecchio_formato_senza_tipo_resta_un_applicazione() {
        let testo =
            "[[configurazione]]\nnome='p'\nadattatore='gdb'\nprogramma='${workspaceFolder}/p'\n";
        let lette = leggi_testo(testo).unwrap();
        assert_eq!(lette[0].tipo, Tipo::Applicazione);
        let lanci = lanci(&lette, &radice()).unwrap();
        assert!(lanci[0].program.is_absolute());
        assert!(lanci[0].program.ends_with("p"));
    }

    #[test]
    fn maven_con_profili_e_goal_e_comando_su_path() {
        let maven = Configurazione {
            profili: vec!["dev".into(), "it".into()],
            goal: vec!["clean".into(), "verify".into()],
            ..Configurazione::nuova(Tipo::Maven, "m".into())
        };
        let risolta = maven.risolvi(&radice(), Some(Path::new("mvn"))).unwrap();
        assert_eq!(risolta.argomenti, ["-B", "-Pdev,it", "clean", "verify"]);
        assert_eq!(risolta.cartella, radice());
        assert!(maven.risolvi(&radice(), None).is_err());
        let comando = Configurazione {
            programma: "python3".into(),
            ..Configurazione::nuova(Tipo::Comando, "c".into())
        };
        assert_eq!(
            comando.risolvi(&radice(), None).unwrap().programma,
            PathBuf::from("python3")
        );
        assert!(comando.lancio(&radice()).is_none());
    }

    #[test]
    fn problemi_espliciti() {
        assert!(
            Configurazione::nuova(Tipo::Comando, "c".into())
                .problema()
                .is_some()
        );
        assert!(
            Configurazione::nuova(Tipo::Maven, " ".into())
                .problema()
                .is_some()
        );
        let attach = Configurazione {
            richiesta: Some("attach".into()),
            ..Configurazione::nuova(Tipo::Applicazione, "a".into())
        };
        assert!(attach.problema().is_some());
        let relativo = Configurazione {
            programma: "target/p".into(),
            ..Configurazione::nuova(Tipo::Applicazione, "a".into())
        };
        assert!(matches!(
            lanci(&[relativo], &radice()),
            Err(LaunchError::Invalid { index: 1, .. })
        ));
    }

    #[test]
    fn argomenti_ambiente_ed_elenchi() {
        let argomenti = dividi_argomenti(r#"-v "due parole" a\"b  "" fine"#);
        assert_eq!(argomenti, ["-v", "due parole", "a\"b", "", "fine"]);
        assert_eq!(dividi_argomenti(&unisci_argomenti(&argomenti)), argomenti);
        let ambiente = dividi_ambiente("A=1; B = due ;").unwrap();
        assert_eq!(ambiente["B"], "due");
        assert_eq!(
            dividi_ambiente(&unisci_ambiente(&ambiente)).unwrap(),
            ambiente
        );
        assert!(dividi_ambiente("senza uguale").is_err());
        assert_eq!(dividi_elenco("dev, it  prod"), ["dev", "it", "prod"]);
    }

    #[test]
    fn scrittura_atomica_e_file_assente_vuoto() {
        let radice = tempfile::tempdir().unwrap();
        assert!(leggi(radice.path()).unwrap().is_empty());
        let configurazioni = vec![Configurazione::nuova(Tipo::Maven, "m".into())];
        scrivi(radice.path(), &configurazioni).unwrap();
        assert_eq!(leggi(radice.path()).unwrap(), configurazioni);
        let residui: Vec<_> = std::fs::read_dir(radice.path().join(".lume"))
            .unwrap()
            .map(|voce| voce.unwrap().file_name())
            .collect();
        assert_eq!(
            residui.len(),
            1,
            "nessun file temporaneo resta: {residui:?}"
        );
    }
}
