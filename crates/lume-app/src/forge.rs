//! Forge: la configurazione di esecuzione — «come avviare il tuo programma».
//!
//! Viene dal prototipo di design (`class Forge`), dove è il pulsante
//! `forge ▾` nella title bar accanto al ▶, e la voce «Forge · configurazioni…»
//! del menù Esegui.
//!
//! **Perché è solo un costruttore di comandi.** Una configurazione qui non
//! avvia niente da sé: produce una riga di comando, e quella riga la esegue il
//! terminale che il progetto ha già. Non c'è un secondo motore di esecuzione,
//! non c'è un runner finto che dice «avviato» senza un processo dietro — la
//! prova che il comando gira è la stessa che si avrebbe scrivendolo a mano nel
//! terminale, perché è letteralmente quella.
//!
//! Per lo stesso motivo il modello contiene **solo quel che la riga di comando
//! può mantenere**: niente cartella di lavoro, variabili d'ambiente, ambiente
//! WSL o passi «prima», perché il terminale li decide da sé (cartella e shell
//! della sessione attiva) e un campo che non cambia il comando è un controllo
//! che mente.
//!
//! Da qui anche la scelta di tenere **otto famiglie** invece delle sole Rust:
//! comporre `dotnet test …` è onesto anche su una macchina senza dotnet, esattamente
//! come lo sarebbe scriverlo nel terminale. Chi non ce l'ha vede l'errore della
//! shell, che è l'informazione giusta — non un controllo assente né uno che
//! finge.
use std::fmt::Write as _;

/// La famiglia di una configurazione: decide il vocabolario e il comando.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    Rust,
    Node,
    Python,
    Jvm,
    Go,
    DotNet,
    Docker,
    Shell,
}

/// Cosa si avvia dentro una famiglia.
///
/// È un enum solo, non uno per famiglia: le varianti non si mescolano mai
/// perché [`Family::kinds`] elenca quelle legittime, e un enum per famiglia
/// vorrebbe otto tipi con lo stesso identico comportamento.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    // Rust
    Bin,
    Test,
    Example,
    // Node
    Script,
    File,
    // Python
    Module,
    Pytest,
    // JVM
    Main,
    Gradle,
    // Go
    Pkg,
    GoTest,
    // .NET
    Project,
    DotNetTest,
    // Docker
    Up,
    Build,
    // Shell
    Cmd,
}

impl Family {
    pub const ALL: [Family; 8] = [
        Family::Rust,
        Family::Node,
        Family::Python,
        Family::Jvm,
        Family::Go,
        Family::DotNet,
        Family::Docker,
        Family::Shell,
    ];

    /// Il nome mostrato nella scelta «Che tipo di programma vuoi avviare?».
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust (Cargo)",
            Self::Node => "Node.js",
            Self::Python => "Python",
            Self::Jvm => "Java / Kotlin",
            Self::Go => "Go",
            Self::DotNet => ".NET",
            Self::Docker => "Docker Compose",
            Self::Shell => "Comando shell",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Rust => "Binari, test ed esempi di un crate Cargo.",
            Self::Node => "Script npm o un file JavaScript/TypeScript.",
            Self::Python => "File, modulo o test pytest.",
            Self::Jvm => "Classe main o task Gradle (./gradlew).",
            Self::Go => "Pacchetto main o test del modulo.",
            Self::DotNet => "Progetto C# o F# con dotnet run.",
            Self::Docker => "Avvia i servizi di supporto (database, code, cache).",
            Self::Shell => "Qualsiasi comando, quando nient’altro calza.",
        }
    }

    /// I tipi legittimi, in ordine; il primo è il default di una nuova
    /// configurazione. Ogni voce porta l'etichetta e il segnaposto del campo
    /// «bersaglio», che cambia nome da famiglia a famiglia.
    pub fn kinds(self) -> &'static [(Kind, &'static str, &'static str)] {
        match self {
            Self::Rust => &[
                (Kind::Bin, "Binario", "nome del binario"),
                (Kind::Test, "Test", "filtro test (vuoto = tutti)"),
                (Kind::Example, "Esempio", "nome dell’esempio"),
            ],
            Self::Node => &[
                (Kind::Script, "Script npm", "dev"),
                (Kind::File, "File", "src/index.ts"),
            ],
            Self::Python => &[
                (Kind::File, "File", "main.py"),
                (Kind::Module, "Modulo", "app.server"),
                (Kind::Pytest, "pytest", "tests/"),
            ],
            Self::Jvm => &[
                (Kind::Main, "Classe main", "com.example.App"),
                (Kind::Gradle, "Task Gradle", "bootRun"),
            ],
            Self::Go => &[
                (Kind::Pkg, "Pacchetto", "./cmd/server"),
                (Kind::GoTest, "Test", "./..."),
            ],
            Self::DotNet => &[
                (Kind::Project, "Progetto", "src/Api/Api.csproj"),
                (Kind::DotNetTest, "Test", "tests/"),
            ],
            Self::Docker => &[
                (Kind::Up, "Avvia servizi", "docker-compose.yml"),
                (Kind::Build, "Ricostruisci", "docker-compose.yml"),
            ],
            Self::Shell => &[(Kind::Cmd, "Comando", "make dev")],
        }
    }

    /// L'etichetta del campo «bersaglio» per un tipo: «Binario», «Filtro
    /// test», «File compose»… Il prototipo la chiama `tl`.
    pub fn target_label(self, kind: Kind) -> &'static str {
        match (self, kind) {
            (Self::Rust, Kind::Bin) => "Binario",
            (Self::Rust, Kind::Test) => "Filtro test",
            (Self::Rust, Kind::Example) => "Esempio",
            (Self::Node, Kind::Script) => "Script",
            (Self::Node, Kind::File) => "File",
            (Self::Python, Kind::File) => "File",
            (Self::Python, Kind::Module) => "Modulo",
            (Self::Python, Kind::Pytest) => "Cartella o file",
            (Self::Jvm, Kind::Main) => "Classe",
            (Self::Jvm, Kind::Gradle) => "Task",
            (Self::Go, Kind::Pkg) => "Pacchetto",
            (Self::Go, Kind::GoTest) => "Pacchetti",
            (Self::DotNet, Kind::Project) => "Progetto",
            (Self::DotNet, Kind::DotNetTest) => "Progetto test",
            (Self::Docker, Kind::Up | Kind::Build) => "File compose",
            (Self::Shell, Kind::Cmd) => "Comando",
            // Un tipo fuori dalla sua famiglia non si costruisce dalla UI, che
            // pesca sempre da `kinds()`. Se ci arriva, l'etichetta generica è
            // preferibile a un panico.
            _ => "Bersaglio",
        }
    }

    /// Il primo tipo della famiglia: quello di una configurazione nuova.
    pub fn default_kind(self) -> Kind {
        // `kinds()` non è mai vuoto per costruzione: ogni braccio del match
        // sopra ha almeno una voce.
        self.kinds().first().map_or(Kind::Cmd, |(k, _, _)| *k)
    }
}

/// La configurazione di esecuzione corrente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    pub family: Family,
    pub kind: Kind,
    /// Il «bersaglio»: binario, script, classe, file compose… Vuoto = da
    /// riempire, e il comando lo mostra come `…`.
    pub target: String,
    pub args: String,
    /// Solo Rust: `--release`.
    pub release: bool,
    /// Solo Rust: le feature di Cargo.
    pub features: String,
}

impl RunConfig {
    /// Una configurazione vuota della famiglia scelta.
    pub fn blank(family: Family) -> Self {
        Self {
            family,
            kind: family.default_kind(),
            target: String::new(),
            args: String::new(),
            release: false,
            features: String::new(),
        }
    }

    /// La riga di comando, esattamente come la compone il prototipo.
    ///
    /// È **una stringa**, non un `Command` già montato: quel che si vede nel
    /// pannello dev'essere quel che finisce nel terminale, carattere per
    /// carattere, altrimenti l'anteprima sarebbe una promessa e non un fatto.
    pub fn command(&self) -> String {
        let t = if self.target.is_empty() {
            "…"
        } else {
            self.target.as_str()
        };
        // Nel prototipo `a` è già preceduto dallo spazio.
        let a = if self.args.is_empty() {
            String::new()
        } else {
            format!(" {}", self.args)
        };

        match self.family {
            Family::Rust => {
                let mut p = String::new();
                if self.release {
                    p.push_str(" --release");
                }
                if !self.features.is_empty() {
                    let _ = write!(p, " --features {}", self.features);
                }
                // `--` separa gli argomenti del programma da quelli di cargo.
                let sep = if self.args.is_empty() {
                    String::new()
                } else {
                    format!(" --{a}")
                };
                match self.kind {
                    Kind::Test => {
                        let bersaglio = if self.target.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", self.target)
                        };
                        format!("cargo test{p}{bersaglio}{sep}")
                    }
                    Kind::Example => format!("cargo run{p} --example {t}{sep}"),
                    // Bin e qualunque altro tipo: il prototipo ricade su run.
                    _ => format!("cargo run{p} --bin {t}{sep}"),
                }
            }
            Family::Node => match self.kind {
                Kind::Script => {
                    let coda = if self.args.is_empty() {
                        String::new()
                    } else {
                        format!(" --{a}")
                    };
                    format!("npm run {t}{coda}")
                }
                _ => format!("node {t}{a}"),
            },
            Family::Python => match self.kind {
                Kind::Module => format!("python -m {t}{a}"),
                Kind::Pytest => format!("pytest {t}{a}"),
                _ => format!("python {t}{a}"),
            },
            Family::Jvm => match self.kind {
                Kind::Gradle => format!("./gradlew {t}{a}"),
                _ => {
                    let args = if self.args.is_empty() {
                        String::new()
                    } else {
                        format!(" --args=\"{}\"", self.args)
                    };
                    format!("./gradlew run -PmainClass={t}{args}")
                }
            },
            Family::Go => match self.kind {
                Kind::GoTest => format!("go test {t}{a}"),
                _ => format!("go run {t}{a}"),
            },
            Family::DotNet => match self.kind {
                Kind::DotNetTest => format!("dotnet test {t}{a}"),
                _ => format!("dotnet run --project {t}{a}"),
            },
            Family::Docker => {
                let azione = if self.kind == Kind::Build {
                    " up --build"
                } else {
                    " up"
                };
                format!("docker compose -f {t}{azione}{a}")
            }
            Family::Shell => format!("{t}{a}"),
        }
    }

    /// Perché questa configurazione non si può eseguire, o `None` se si può.
    ///
    /// Due ragioni sole. Un carattere di controllo (a capo, tabulazione, ESC,
    /// Ctrl+C…) in un campo: il testo finisce nel terminale seguito da Invio,
    /// e un a capo incollato lo trasformerebbe in due comandi. E un bersaglio
    /// vuoto: `cargo run --bin …` non è un comando. L'eccezione è
    /// `cargo test`, dove «vuoto = tutti» è la semantica dichiarata.
    pub fn problem(&self) -> Option<String> {
        let campi = [&self.target, &self.args, &self.features];
        if campi
            .iter()
            .any(|campo| campo.chars().any(char::is_control))
        {
            return Some(
                "Un campo contiene un a capo o un altro carattere di controllo.".to_owned(),
            );
        }
        let test_di_cargo = self.family == Family::Rust && self.kind == Kind::Test;
        if self.target.trim().is_empty() && !test_di_cargo {
            return Some(format!(
                "Manca il campo «{}».",
                self.family.target_label(self.kind)
            ));
        }
        None
    }

    pub fn is_runnable(&self) -> bool {
        self.problem().is_none()
    }

    /// Quel che si scrive nel terminale: il comando e l'Invio (`\r`, lo stesso
    /// byte che manda il tasto). `None` se la configurazione non si esegue.
    pub fn terminal_input(&self) -> Option<Vec<u8>> {
        self.is_runnable()
            .then(|| format!("{}\r", self.command()).into_bytes())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;

    fn rust(kind: Kind, target: &str, args: &str) -> RunConfig {
        RunConfig {
            target: target.to_owned(),
            args: args.to_owned(),
            kind,
            ..RunConfig::blank(Family::Rust)
        }
    }

    /// Le tre configurazioni d'esempio del prototipo, comando per comando.
    /// Sono il contratto: se cambiano qui, la vista mente all'utente.
    #[test]
    fn i_comandi_del_prototipo_si_compongono_uguali() {
        let c1 = rust(Kind::Bin, "forge", "--port 8080");
        assert_eq!(c1.command(), "cargo run --bin forge -- --port 8080");

        let c2 = rust(Kind::Test, "remote::", "--nocapture");
        assert_eq!(c2.command(), "cargo test remote:: -- --nocapture");

        let mut c3 = RunConfig::blank(Family::Docker);
        c3.target = "docker-compose.yml".to_owned();
        assert_eq!(c3.command(), "docker compose -f docker-compose.yml up");
    }

    #[test]
    fn release_e_features_entrano_prima_del_bersaglio() {
        let mut c = rust(Kind::Bin, "lume", "");
        c.release = true;
        c.features = "sql".to_owned();
        assert_eq!(c.command(), "cargo run --release --features sql --bin lume");
    }

    /// Senza argomenti non compare il `--` di separazione: sarebbe rumore, e
    /// per `cargo test` cambierebbe pure il significato.
    #[test]
    fn senza_argomenti_niente_doppio_trattino() {
        assert_eq!(rust(Kind::Test, "", "").command(), "cargo test");
        assert_eq!(
            rust(Kind::Bin, "lume", "").command(),
            "cargo run --bin lume"
        );
    }

    /// Un bersaglio vuoto si vede: il comando mostra `…` invece di lasciare
    /// un buco che sembrerebbe valido.
    #[test]
    fn un_bersaglio_vuoto_si_vede_nel_comando() {
        assert_eq!(rust(Kind::Bin, "", "").command(), "cargo run --bin …");
        let mut c = RunConfig::blank(Family::Shell);
        c.args = "-la".to_owned();
        assert_eq!(c.command(), "… -la");
    }

    #[test]
    fn ogni_famiglia_compone_il_suo_comando() {
        let casi: [(Family, Kind, &str, &str, &str); 8] = [
            (Family::Rust, Kind::Bin, "app", "", "cargo run --bin app"),
            (Family::Node, Kind::File, "src/i.ts", "", "node src/i.ts"),
            (
                Family::Python,
                Kind::Module,
                "app.server",
                "",
                "python -m app.server",
            ),
            (
                Family::Jvm,
                Kind::Gradle,
                "bootRun",
                "",
                "./gradlew bootRun",
            ),
            (Family::Go, Kind::Pkg, "./cmd/s", "", "go run ./cmd/s"),
            (
                Family::DotNet,
                Kind::Project,
                "a.csproj",
                "",
                "dotnet run --project a.csproj",
            ),
            (
                Family::Docker,
                Kind::Build,
                "c.yml",
                "",
                "docker compose -f c.yml up --build",
            ),
            (Family::Shell, Kind::Cmd, "make dev", "", "make dev"),
        ];
        for (family, kind, target, args, atteso) in casi {
            let config = RunConfig {
                family,
                kind,
                target: target.to_owned(),
                args: args.to_owned(),
                ..RunConfig::blank(family)
            };
            assert_eq!(config.command(), atteso, "famiglia {family:?}");
        }
    }

    /// `npm run` e `gradlew run` passano gli argomenti con una sintassi loro.
    #[test]
    fn npm_e_gradle_passano_gli_argomenti_a_modo_loro() {
        let mut npm = RunConfig::blank(Family::Node);
        npm.kind = Kind::Script;
        npm.target = "dev".to_owned();
        npm.args = "--porta 3000".to_owned();
        assert_eq!(npm.command(), "npm run dev -- --porta 3000");

        let mut jvm = RunConfig::blank(Family::Jvm);
        jvm.target = "com.example.App".to_owned();
        jvm.args = "--debug".to_owned();
        assert_eq!(
            jvm.command(),
            "./gradlew run -PmainClass=com.example.App --args=\"--debug\""
        );
    }

    /// Ogni famiglia ha almeno un tipo, e ogni tipo elencato ha la sua
    /// etichetta del bersaglio — niente ricade sul generico.
    #[test]
    fn ogni_tipo_elencato_ha_la_sua_etichetta() {
        for family in Family::ALL {
            let kinds = family.kinds();
            assert!(!kinds.is_empty(), "{family:?} senza tipi");
            for (kind, _, _) in kinds {
                assert_ne!(
                    family.target_label(*kind),
                    "Bersaglio",
                    "{family:?}/{kind:?} ricade sull'etichetta generica"
                );
            }
        }
    }

    /// Un a capo incollato nel bersaglio diventerebbe un secondo comando,
    /// eseguito con lo stesso Invio: la configurazione non si esegue.
    #[test]
    fn un_campo_con_un_a_capo_o_un_carattere_di_controllo_non_si_esegue() {
        let mut c = RunConfig::blank(Family::Shell);
        c.target = "echo uno\ngit push".to_owned();
        assert!(!c.is_runnable());
        assert!(c.terminal_input().is_none());

        for controllo in ["\r", "\t", "\u{1b}", "\u{3}"] {
            let mut c = rust(Kind::Bin, "lume", "");
            c.args = format!("--x{controllo}y");
            assert!(
                !c.is_runnable(),
                "carattere di controllo {controllo:?} negli argomenti"
            );

            let mut c = rust(Kind::Bin, "lume", "");
            c.release = true;
            c.features = format!("sql{controllo}");
            assert!(
                !c.is_runnable(),
                "carattere di controllo {controllo:?} nelle feature"
            );
        }
        assert!(rust(Kind::Bin, "lume", "--port 8080").is_runnable());
    }

    /// Senza bersaglio non c'è niente da avviare; l'eccezione è `cargo test`,
    /// dove «vuoto = tutti» è proprio quel che dice il segnaposto.
    #[test]
    fn un_bersaglio_vuoto_non_si_esegue_tranne_i_test_di_cargo() {
        assert!(!rust(Kind::Bin, "", "").is_runnable());
        assert!(!rust(Kind::Bin, "   ", "").is_runnable());
        assert!(!RunConfig::blank(Family::Shell).is_runnable());
        assert!(rust(Kind::Test, "", "").is_runnable());
        for family in Family::ALL {
            let c = RunConfig::blank(family);
            assert_eq!(
                c.is_runnable(),
                family == Family::Rust && c.kind == Kind::Test,
                "{family:?}: solo cargo test parte senza bersaglio, e Rust nasce sul binario"
            );
        }
    }

    /// Il motivo mostrato all'utente nomina il campo giusto, come lo chiama
    /// il riquadro.
    #[test]
    fn il_motivo_del_rifiuto_nomina_il_campo() {
        assert_eq!(
            rust(Kind::Bin, "", "").problem().as_deref(),
            Some("Manca il campo «Binario».")
        );
        assert_eq!(
            RunConfig::blank(Family::Docker).problem().as_deref(),
            Some("Manca il campo «File compose».")
        );
        let mut c = RunConfig::blank(Family::Shell);
        c.target = "a\nb".to_owned();
        assert!(c.problem().unwrap().contains("carattere di controllo"));
        assert_eq!(rust(Kind::Bin, "lume", "").problem(), None);
    }

    /// Quel che finisce nel terminale è l'anteprima, più l'Invio.
    #[test]
    fn il_comando_va_al_terminale_con_l_invio() {
        let mut c = rust(Kind::Bin, "lume", "--port 8080");
        c.release = true;
        assert_eq!(
            c.terminal_input(),
            Some(b"cargo run --release --bin lume -- --port 8080\r".to_vec())
        );
        assert_eq!(rust(Kind::Bin, "", "").terminal_input(), None);
    }

    /// Le descrizioni non promettono quel che il comando non fa.
    #[test]
    fn le_descrizioni_non_promettono_maven_ne_ambienti_virtuali() {
        for family in Family::ALL {
            let d = family.description();
            assert!(!d.contains("Maven"), "{family:?}: {d}");
            assert!(!d.contains("virtuale"), "{family:?}: {d}");
        }
    }
}
