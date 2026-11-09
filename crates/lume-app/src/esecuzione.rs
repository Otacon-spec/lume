//! Configurazioni di esecuzione del widget «Esegui» (piano 17), sul modello
//! di IntelliJ: «File corrente», le configurazioni di `.lume/launch.toml` e i
//! goal Maven di un progetto con `pom.xml`.
//!
//! Tutto qui è puro: la ricerca degli strumenti (solo `stat`, nessun
//! processo) avviene in [`Strumenti::cerca`], chiamata fuori dal thread
//! della UI insieme alla lettura di `.lume/launch.toml`. Zero-mock: una
//! configurazione esiste solo se lo strumento che la esegue è stato trovato;
//! ▶ e 🐞 compaiono solo se possono agire.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lume_dap::{AdapterId, LaunchConfig};

use crate::tools::terminal::RunSpec;

/// Gli eseguibili trovati su `PATH` (cartelle assolute, file esistenti).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Strumenti {
    pub python: Option<PathBuf>,
    pub node: Option<PathBuf>,
    pub java: Option<PathBuf>,
    pub cc: Option<PathBuf>,
    pub mvn: Option<PathBuf>,
    pub gdb: Option<PathBuf>,
}

fn cerca_in(nomi: &[&str], cartelle: &[PathBuf]) -> Option<PathBuf> {
    cartelle
        .iter()
        .filter(|cartella| cartella.is_absolute())
        .flat_map(|cartella| nomi.iter().map(move |nome| cartella.join(nome)))
        .find(|candidato| candidato.is_file())
}

fn eseguibile(nome: &str) -> String {
    if cfg!(windows) {
        format!("{nome}.exe")
    } else {
        nome.to_owned()
    }
}

impl Strumenti {
    pub fn cerca_in(cartelle: &[PathBuf]) -> Self {
        let uno = |nome: &str| cerca_in(&[&eseguibile(nome)], cartelle);
        Self {
            python: uno("python3").or_else(|| uno("python")),
            node: uno("node"),
            java: uno("java"),
            cc: uno("cc").or_else(|| uno("gcc")).or_else(|| uno("clang")),
            mvn: if cfg!(windows) {
                cerca_in(&["mvn.cmd", "mvn.bat"], cartelle)
            } else {
                cerca_in(&["mvn"], cartelle)
            },
            gdb: AdapterId::Gdb.find_in(cartelle),
        }
    }

    pub fn cerca() -> Self {
        let cartelle: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|valore| std::env::split_paths(&valore).collect())
            .unwrap_or_default();
        Self::cerca_in(&cartelle)
    }
}

/// Un progetto Maven nella radice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Maven {
    /// `mvnw` del progetto se c'è, altrimenti `mvn` di sistema.
    pub comando: PathBuf,
    pub wrapper: bool,
    /// Il `pom.xml` dichiara exec-maven-plugin: si offre anche `exec:java`.
    pub exec: bool,
}

/// Limite di lettura di `pom.xml`: è configurazione.
pub const MAX_POM_BYTES: u64 = 1024 * 1024;

/// Cerca un progetto Maven in `radice` (I/O bloccante: fuori dalla UI).
pub fn cerca_maven(radice: &Path, strumenti: &Strumenti) -> Option<Maven> {
    let pom = radice.join("pom.xml");
    let metadati = std::fs::metadata(&pom).ok()?;
    if !metadati.is_file() {
        return None;
    }
    let exec = metadati.len() <= MAX_POM_BYTES
        && std::fs::read_to_string(&pom).is_ok_and(|testo| testo.contains("exec-maven-plugin"));
    let wrapper = radice.join(if cfg!(windows) { "mvnw.cmd" } else { "mvnw" });
    if wrapper.is_file() {
        return Some(Maven {
            comando: wrapper,
            wrapper: true,
            exec,
        });
    }
    strumenti.mvn.clone().map(|comando| Maven {
        comando,
        wrapper: false,
        exec,
    })
}

/// La configurazione scelta nel widget.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Scelta {
    #[default]
    FileCorrente,
    Lancio(String),
    Maven(&'static str),
}

pub const GOAL_MAVEN: [&str; 3] = ["compile", "test", "package"];
pub const GOAL_EXEC: &str = "exec:java";

/// Che cosa fa «File corrente» per un'estensione.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileCorrente {
    /// Interprete e file: `python3 x.py`, `node x.js`, `java X.java`.
    Interpretato { programma: PathBuf, file: PathBuf },
    /// C: si compila con `cc -g -O0`, poi si esegue o si debugga con gdb.
    C { cc: PathBuf, file: PathBuf },
}

/// «File corrente» per il file attivo, se c'è uno strumento che lo esegue.
pub fn file_corrente(file: &Path, strumenti: &Strumenti) -> Option<FileCorrente> {
    let estensione = file.extension()?.to_str()?.to_ascii_lowercase();
    let interpretato = |programma: &Option<PathBuf>| {
        programma
            .clone()
            .map(|programma| FileCorrente::Interpretato {
                programma,
                file: file.to_path_buf(),
            })
    };
    match estensione.as_str() {
        "py" => interpretato(&strumenti.python),
        "js" | "mjs" | "cjs" => interpretato(&strumenti.node),
        // Lancio di un sorgente singolo, JDK 11 e successivi (JEP 330).
        "java" => interpretato(&strumenti.java),
        "c" => strumenti.cc.clone().map(|cc| FileCorrente::C {
            cc,
            file: file.to_path_buf(),
        }),
        _ => None,
    }
}

/// Dove finisce l'eseguibile compilato da «File corrente» per un file C.
pub fn binario_c(file: &Path) -> PathBuf {
    let nome = file.file_stem().map_or_else(
        || "programma".to_owned(),
        |nome| nome.to_string_lossy().into_owned(),
    );
    std::env::temp_dir()
        .join(format!("lume-esegui-{}", std::process::id()))
        .join(nome)
}

/// Gli argomenti di `cc` per compilare `file` con i simboli di debug.
pub fn argomenti_cc(file: &Path, uscita: &Path) -> Vec<String> {
    vec![
        "-g".into(),
        "-O0".into(),
        "-o".into(),
        uscita.to_string_lossy().into_owned(),
        file.to_string_lossy().into_owned(),
    ]
}

/// Una voce del menu del widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voce {
    pub scelta: Scelta,
    pub etichetta: String,
}

/// Le voci offerte, nell'ordine di IntelliJ: file corrente, poi le
/// configurazioni del progetto.
pub fn voci(
    configurazioni: &[crate::configurazioni::Configurazione],
    maven: Option<&Maven>,
) -> Vec<Voce> {
    let mut voci = vec![Voce {
        scelta: Scelta::FileCorrente,
        etichetta: "File corrente".into(),
    }];
    voci.extend(configurazioni.iter().map(|configurazione| Voce {
        scelta: Scelta::Lancio(configurazione.nome.clone()),
        etichetta: configurazione.nome.clone(),
    }));
    if let Some(maven) = maven {
        let mut goal: Vec<&'static str> = GOAL_MAVEN.to_vec();
        if maven.exec {
            goal.push(GOAL_EXEC);
        }
        voci.extend(goal.into_iter().map(|goal| Voce {
            scelta: Scelta::Maven(goal),
            etichetta: format!("Maven: {goal}"),
        }));
    }
    voci
}

/// Il comando di ▶ per una configurazione di `.lume/launch.toml`.
pub fn esegui_configurazione(
    configurazione: &crate::configurazioni::Configurazione,
    radice: &Path,
    maven: Option<&Maven>,
) -> Result<RunSpec, String> {
    let risolta = configurazione.risolvi(radice, maven.map(|maven| maven.comando.as_path()))?;
    Ok(RunSpec {
        label: configurazione.nome.trim().to_owned(),
        program: risolta.programma,
        args: risolta.argomenti,
        cwd: risolta.cartella,
        env: risolta.ambiente,
    })
}

/// Il comando di ▶ per un goal Maven.
pub fn esegui_maven(maven: &Maven, goal: &str, radice: &Path) -> RunSpec {
    RunSpec {
        label: format!("Maven: {goal}"),
        program: maven.comando.clone(),
        // -B: senza colori ANSI interattivi né richieste di input.
        args: vec!["-B".into(), goal.to_owned()],
        cwd: radice.to_path_buf(),
        env: Vec::new(),
    }
}

/// Il comando di ▶ per un file interpretato.
pub fn esegui_interpretato(programma: &Path, file: &Path) -> RunSpec {
    let cartella = file
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    RunSpec {
        label: file.file_name().map_or_else(
            || "file".to_owned(),
            |nome| nome.to_string_lossy().into_owned(),
        ),
        program: programma.to_path_buf(),
        args: vec![file.to_string_lossy().into_owned()],
        cwd: cartella,
        env: Vec::new(),
    }
}

/// La configurazione di debug per un binario C appena compilato.
pub fn lancio_c(file: &Path, binario: &Path) -> LaunchConfig {
    LaunchConfig {
        name: file.file_name().map_or_else(
            || "file".to_owned(),
            |nome| nome.to_string_lossy().into_owned(),
        ),
        adapter: AdapterId::Gdb,
        program: binario.to_path_buf(),
        args: Vec::new(),
        cwd: file
            .parent()
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf),
        env: BTreeMap::new(),
        stop_at_entry: false,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn cartella_con(nomi: &[&str]) -> tempfile::TempDir {
        let cartella = tempfile::tempdir().unwrap();
        for nome in nomi {
            // Su Windows Maven è uno script `mvn.cmd`, non un `.exe`.
            let file = if *nome == "mvn" && cfg!(windows) {
                "mvn.cmd".to_owned()
            } else {
                eseguibile(nome)
            };
            std::fs::write(cartella.path().join(file), b"").unwrap();
        }
        cartella
    }

    #[test]
    fn file_corrente_solo_con_lo_strumento_che_lo_esegue() {
        let bin = cartella_con(&["python3", "cc"]);
        let strumenti = Strumenti::cerca_in(&[bin.path().to_path_buf()]);
        assert!(matches!(
            file_corrente(Path::new("/p/a.py"), &strumenti),
            Some(FileCorrente::Interpretato { .. })
        ));
        assert!(matches!(
            file_corrente(Path::new("/p/a.c"), &strumenti),
            Some(FileCorrente::C { .. })
        ));
        assert_eq!(file_corrente(Path::new("/p/A.java"), &strumenti), None);
        assert_eq!(file_corrente(Path::new("/p/a.rs"), &strumenti), None);
        assert_eq!(file_corrente(Path::new("/p/senza"), &strumenti), None);
    }

    #[test]
    fn maven_preferisce_il_wrapper_e_riconosce_exec() {
        let bin = cartella_con(&["mvn"]);
        let strumenti = Strumenti::cerca_in(&[bin.path().to_path_buf()]);
        let progetto = tempfile::tempdir().unwrap();
        assert_eq!(cerca_maven(progetto.path(), &strumenti), None);
        std::fs::write(progetto.path().join("pom.xml"), "<project/>").unwrap();
        let maven = cerca_maven(progetto.path(), &strumenti).unwrap();
        assert!(!maven.wrapper && !maven.exec);
        assert_eq!(voci(&[], Some(&maven)).len(), 1 + GOAL_MAVEN.len());
        std::fs::write(
            progetto.path().join("pom.xml"),
            "<project><build><plugins><plugin><artifactId>exec-maven-plugin</artifactId></plugin></plugins></build></project>",
        )
        .unwrap();
        let nome = if cfg!(windows) { "mvnw.cmd" } else { "mvnw" };
        std::fs::write(progetto.path().join(nome), b"").unwrap();
        let maven = cerca_maven(progetto.path(), &strumenti).unwrap();
        assert!(maven.wrapper && maven.exec);
        let etichette: Vec<_> = voci(&[], Some(&maven))
            .into_iter()
            .map(|v| v.etichetta)
            .collect();
        assert_eq!(
            etichette.last().map(String::as_str),
            Some("Maven: exec:java")
        );
        let spec = esegui_maven(&maven, "test", progetto.path());
        assert_eq!(spec.args, ["-B", "test"]);
    }

    #[test]
    fn senza_mvn_ne_wrapper_maven_non_compare() {
        let progetto = tempfile::tempdir().unwrap();
        std::fs::write(progetto.path().join("pom.xml"), "<project/>").unwrap();
        assert_eq!(cerca_maven(progetto.path(), &Strumenti::default()), None);
    }
}

// ── Il pom.xml per il pannello Maven ────────────────────────────────

/// Quel che il pannello Maven mostra di un `pom.xml`, letto senza avviare
/// Maven (niente dipendenze risolte, niente POM effettivo).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pom {
    pub group_id: Option<String>,
    pub artifact_id: Option<String>,
    pub version: Option<String>,
    pub packaging: Option<String>,
    pub moduli: Vec<String>,
    pub profili: Vec<String>,
    /// `groupId:artifactId` dei plugin dichiarati in `build/plugins`.
    pub plugin: Vec<String>,
}

fn figlio<'a>(nodo: roxmltree::Node<'a, 'a>, nome: &str) -> Option<roxmltree::Node<'a, 'a>> {
    nodo.children().find(|figlio| figlio.has_tag_name(nome))
}

fn testo_figlio(nodo: roxmltree::Node<'_, '_>, nome: &str) -> Option<String> {
    figlio(nodo, nome)
        .and_then(|figlio| figlio.text())
        .map(str::trim)
        .filter(|testo| !testo.is_empty())
        .map(str::to_owned)
}

/// Interpreta il testo di un `pom.xml` (DTD rifiutati da roxmltree).
pub fn leggi_pom(testo: &str) -> Result<Pom, String> {
    let documento = roxmltree::Document::parse(testo)
        .map_err(|errore| format!("pom.xml non valido: {errore}"))?;
    let progetto = documento.root_element();
    if !progetto.has_tag_name("project") {
        return Err("pom.xml senza elemento <project>".into());
    }
    let elenco =
        |padre: Option<roxmltree::Node<'_, '_>>, voce: &str, campo: Option<&str>| -> Vec<String> {
            padre
                .map(|padre| {
                    padre
                        .children()
                        .filter(|figlio| figlio.has_tag_name(voce))
                        .filter_map(|figlio| match campo {
                            Some(campo) => testo_figlio(figlio, campo),
                            None => figlio.text().map(str::trim).map(str::to_owned),
                        })
                        .filter(|testo| !testo.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
    let plugin = figlio(progetto, "build")
        .and_then(|build| figlio(build, "plugins"))
        .map(|plugins| {
            plugins
                .children()
                .filter(|figlio| figlio.has_tag_name("plugin"))
                .filter_map(|plugin| {
                    let artefatto = testo_figlio(plugin, "artifactId")?;
                    let gruppo = testo_figlio(plugin, "groupId")
                        .unwrap_or_else(|| "org.apache.maven.plugins".to_owned());
                    Some(format!("{gruppo}:{artefatto}"))
                })
                .collect()
        })
        .unwrap_or_default();
    let genitore = figlio(progetto, "parent");
    Ok(Pom {
        group_id: testo_figlio(progetto, "groupId")
            .or_else(|| genitore.and_then(|g| testo_figlio(g, "groupId"))),
        artifact_id: testo_figlio(progetto, "artifactId"),
        version: testo_figlio(progetto, "version")
            .or_else(|| genitore.and_then(|g| testo_figlio(g, "version"))),
        packaging: testo_figlio(progetto, "packaging"),
        moduli: elenco(figlio(progetto, "modules"), "module", None),
        profili: elenco(figlio(progetto, "profiles"), "profile", Some("id")),
        plugin,
    })
}

/// Legge `pom.xml` dalla radice, se c'è (I/O bloccante, ≤ [`MAX_POM_BYTES`]).
pub fn carica_pom(radice: &Path) -> Option<Result<Pom, String>> {
    let percorso = radice.join("pom.xml");
    let metadati = std::fs::metadata(&percorso)
        .ok()
        .filter(std::fs::Metadata::is_file)?;
    if metadati.len() > MAX_POM_BYTES {
        return Some(Err("pom.xml oltre 1 MiB".into()));
    }
    Some(
        std::fs::read_to_string(&percorso)
            .map_err(|errore| errore.to_string())
            .and_then(|testo| leggi_pom(&testo)),
    )
}

#[cfg(test)]
mod pom_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn pom_con_genitore_moduli_profili_e_plugin() {
        let pom = leggi_pom(
            r#"<project xmlns="http://maven.apache.org/POM/4.0.0">
  <parent><groupId>org.esempio</groupId><version>2.0</version></parent>
  <artifactId>app</artifactId><packaging>pom</packaging>
  <modules><module>core</module><module>web</module></modules>
  <profiles><profile><id>dev</id></profile><profile><id>it</id></profile></profiles>
  <build><plugins>
    <plugin><artifactId>maven-surefire-plugin</artifactId></plugin>
    <plugin><groupId>org.codehaus.mojo</groupId><artifactId>exec-maven-plugin</artifactId></plugin>
  </plugins></build>
</project>"#,
        )
        .unwrap();
        assert_eq!(pom.group_id.as_deref(), Some("org.esempio"));
        assert_eq!(pom.version.as_deref(), Some("2.0"));
        assert_eq!(pom.moduli, ["core", "web"]);
        assert_eq!(pom.profili, ["dev", "it"]);
        assert_eq!(
            pom.plugin,
            [
                "org.apache.maven.plugins:maven-surefire-plugin",
                "org.codehaus.mojo:exec-maven-plugin"
            ]
        );
    }

    #[test]
    fn pom_non_valido_o_con_dtd_e_un_errore() {
        assert!(leggi_pom("<project>").is_err());
        assert!(leggi_pom("<altro/>").is_err());
        assert!(leggi_pom(r#"<!DOCTYPE p [<!ENTITY x "y">]><project/>"#).is_err());
    }
}
