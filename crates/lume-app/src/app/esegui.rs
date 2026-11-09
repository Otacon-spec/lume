//! Il widget «Esegui» nella barra del titolo (piano 17): scelta della
//! configurazione, ▶ in una scheda del Terminale, 🐞 verso il debugger.

use super::*;
use crate::esecuzione::{self, FileCorrente, Scelta, Voce};
use crate::titlebar::{EseguiAzione, RunWidget};
use lume_ui::DebugAction;

/// Esito della compilazione di un file C per «File corrente».
#[derive(Debug, Clone)]
pub struct Compilato {
    pub file: PathBuf,
    pub binario: PathBuf,
    pub debug: bool,
    pub esito: Result<(), String>,
}

const TEMPLATE_LANCIO: &str = "# Configurazioni di esecuzione e debug di Lume.\n\
# Ogni [[configurazione]] compare nel widget Esegui della barra del titolo.\n\
\n\
[[configurazione]]\n\
nome = \"programma\"\n\
adattatore = \"gdb\"\n\
programma = \"${workspaceFolder}/programma\"\n\
argomenti = []\n";

impl Lume {
    /// Le voci del selettore per lo stato corrente.
    pub(super) fn voci_esegui(&self) -> Vec<Voce> {
        let setup = self
            .debug
            .setup
            .as_ref()
            .filter(|_| self.debug.setup_root == self.project_root);
        let configurazioni = setup.map_or(&[][..], |setup| setup.configurazioni.as_slice());
        esecuzione::voci(configurazioni, setup.and_then(|setup| setup.maven.as_ref()))
    }

    pub(super) fn sync_voci_esegui(&mut self) {
        let voci = self.voci_esegui();
        if self.debug.scelta_esegui >= voci.len() {
            self.debug.scelta_esegui = 0;
        }
        self.debug.etichette_esegui = voci.into_iter().map(|voce| voce.etichetta).collect();
    }

    fn scelta_esegui(&self) -> Scelta {
        self.voci_esegui()
            .into_iter()
            .nth(self.debug.scelta_esegui)
            .map(|voce| voce.scelta)
            .unwrap_or_default()
    }

    fn setup_corrente(&self) -> Option<&crate::debug_mode::Setup> {
        self.debug
            .setup
            .as_ref()
            .filter(|_| self.debug.setup_root == self.project_root)
    }

    fn configurazione(&self, nome: &str) -> Option<&crate::configurazioni::Configurazione> {
        self.setup_corrente()?
            .configurazioni
            .iter()
            .find(|configurazione| configurazione.nome == nome)
    }

    /// Il Maven da usare: quello del progetto (`mvnw` o `mvn` con pom.xml),
    /// altrimenti il `mvn` di sistema per le configurazioni Maven scritte a mano.
    fn maven(&self) -> Option<esecuzione::Maven> {
        let setup = self.setup_corrente()?;
        setup.maven.clone().or_else(|| {
            setup
                .strumenti
                .mvn
                .clone()
                .map(|comando| esecuzione::Maven {
                    comando,
                    wrapper: false,
                    exec: false,
                })
        })
    }

    fn strumenti(&self) -> Option<&esecuzione::Strumenti> {
        self.debug
            .setup
            .as_ref()
            .filter(|_| self.debug.setup_root == self.project_root)
            .map(|setup| &setup.strumenti)
    }

    /// Che cosa sanno fare ▶ e 🐞 per la scelta corrente (senza I/O).
    fn capacita_esegui(&self) -> (bool, bool) {
        let gdb = self.strumenti().is_some_and(|s| s.gdb.is_some());
        match self.scelta_esegui() {
            Scelta::FileCorrente => match self
                .active_path()
                .zip(self.strumenti())
                .and_then(|(file, strumenti)| esecuzione::file_corrente(file, strumenti))
            {
                Some(FileCorrente::C { .. }) => (true, gdb),
                Some(FileCorrente::Interpretato { .. }) => (true, false),
                None => (false, false),
            },
            Scelta::Lancio(nome) => match self.configurazione(&nome).map(|c| c.tipo) {
                Some(crate::configurazioni::Tipo::Applicazione) => (true, gdb),
                Some(crate::configurazioni::Tipo::Maven) => (self.maven().is_some(), false),
                Some(crate::configurazioni::Tipo::Comando) => (true, false),
                None => (false, false),
            },
            Scelta::Maven(_) => (true, false),
        }
    }

    /// Il widget per la barra del titolo, se c'è un progetto.
    pub(super) fn run_widget(&self) -> Option<RunWidget<'_>> {
        self.project_root.as_ref()?;
        if !self.feature_is_enabled(crate::features::Feature::Debugger) {
            return None;
        }
        let (esegui, debug) = self.capacita_esegui();
        let vivo = self.debug.is_live();
        Some(RunWidget {
            voci: &self.debug.etichette_esegui,
            scelta: self.debug.scelta_esegui,
            esegui,
            debug: debug && !vivo,
            ferma: vivo,
        })
    }

    pub(super) fn esegui_azione(&mut self, azione: EseguiAzione) -> Task<Message> {
        match azione {
            EseguiAzione::Scegli(indice) => {
                if indice < self.debug.etichette_esegui.len() {
                    self.debug.scelta_esegui = indice;
                }
                Task::none()
            }
            EseguiAzione::Esegui => self.esegui(false),
            EseguiAzione::Debug => self.esegui(true),
            EseguiAzione::Ferma => self.stop_debug(),
            EseguiAzione::AggiornaConfigurazioni => self.load_debug_setup(),
            EseguiAzione::ModificaConfigurazioni => self.apri_configurazioni(),
        }
    }

    fn esegui(&mut self, debug: bool) -> Task<Message> {
        let Some(root) = self.project_root.clone() else {
            return self.notify_error("Esegui: apri prima una cartella di progetto".into());
        };
        if self.strumenti().is_none() {
            let task = self.load_debug_setup();
            return Task::batch([
                task,
                self.notify(
                    NoticeKind::Info,
                    "Esegui: lettura delle configurazioni, riprova fra un attimo".into(),
                ),
            ]);
        }
        match self.scelta_esegui() {
            Scelta::FileCorrente => self.esegui_file_corrente(debug),
            Scelta::Lancio(nome) => {
                let Some(configurazione) = self.configurazione(&nome).cloned() else {
                    return self
                        .notify_error(format!("Esegui: configurazione «{nome}» non trovata"));
                };
                if debug {
                    let Some(indice) = self
                        .debug
                        .configs()
                        .iter()
                        .position(|c| c.name == nome.trim())
                    else {
                        return self.notify_error(format!(
                            "Debug: «{nome}» non è un'applicazione valida da debuggare"
                        ));
                    };
                    self.debug.selected_config = indice;
                    self.debug.override_config = None;
                    return self.avvia_debug_dal_widget();
                }
                let maven = self.maven();
                match esecuzione::esegui_configurazione(&configurazione, &root, maven.as_ref()) {
                    Ok(spec) => {
                        if spec.program.is_absolute() && !spec.program.is_file() {
                            return self.notify_error(format!(
                                "Esegui: {} non esiste: compila prima il programma",
                                spec.program.display()
                            ));
                        }
                        self.esegui_nel_terminale(spec)
                    }
                    Err(problema) => self.notify_error(format!("Esegui «{nome}»: {problema}")),
                }
            }
            Scelta::Maven(goal) => {
                let maven = self
                    .debug
                    .setup
                    .as_ref()
                    .and_then(|setup| setup.maven.clone());
                match maven {
                    Some(maven) => {
                        self.esegui_nel_terminale(esecuzione::esegui_maven(&maven, goal, &root))
                    }
                    None => {
                        self.notify_error("Esegui: nessun progetto Maven (pom.xml e mvn)".into())
                    }
                }
            }
        }
    }

    fn esegui_file_corrente(&mut self, debug: bool) -> Task<Message> {
        let Some(file) = self.active_path().map(Path::to_path_buf) else {
            return self.notify_error("Esegui: nessun file aperto".into());
        };
        let Some(modo) = self
            .strumenti()
            .and_then(|strumenti| esecuzione::file_corrente(&file, strumenti))
        else {
            return self.notify_error(format!(
                "Esegui: nessuno strumento su PATH esegue {}",
                file.display()
            ));
        };
        let sporco = self
            .active_doc()
            .and_then(|id| self.workspace.get(id))
            .is_some_and(lume_core::Document::is_dirty);
        let avviso = sporco.then(|| {
            self.notify(
                NoticeKind::Info,
                "Il file ha modifiche non salvate: si esegue la versione su disco".into(),
            )
        });
        let task = match modo {
            FileCorrente::Interpretato { programma, file } => {
                if debug {
                    return self
                        .notify_error("Debug: nessun adattatore per questo linguaggio".into());
                }
                self.esegui_nel_terminale(esecuzione::esegui_interpretato(&programma, &file))
            }
            FileCorrente::C { cc, file } => {
                let binario = esecuzione::binario_c(&file);
                let argomenti = esecuzione::argomenti_cc(&file, &binario);
                Task::perform(
                    async move {
                        let lavoro = {
                            let binario = binario.clone();
                            tokio::task::spawn_blocking(move || compila(&cc, &argomenti, &binario))
                        };
                        let esito = lavoro
                            .await
                            .unwrap_or_else(|errore| Err(errore.to_string()));
                        Compilato {
                            file,
                            binario,
                            debug,
                            esito,
                        }
                    },
                    |compilato| Message::EseguiCompilato(Box::new(compilato)),
                )
            }
        };
        match avviso {
            Some(avviso) => Task::batch([avviso, task]),
            None => task,
        }
    }

    pub(super) fn esegui_compilato(&mut self, compilato: Compilato) -> Task<Message> {
        if let Err(errore) = compilato.esito {
            return self.notify_error(format!("Compilazione fallita: {errore}"));
        }
        if compilato.debug {
            self.debug.override_config =
                Some(esecuzione::lancio_c(&compilato.file, &compilato.binario));
            return self.avvia_debug_dal_widget();
        }
        let cartella = compilato
            .file
            .parent()
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        let etichetta = compilato.file.file_name().map_or_else(
            || "programma".to_owned(),
            |nome| nome.to_string_lossy().into_owned(),
        );
        self.esegui_nel_terminale(crate::tools::terminal::RunSpec {
            label: etichetta,
            program: compilato.binario,
            args: Vec::new(),
            cwd: cartella,
            env: Vec::new(),
        })
    }

    fn avvia_debug_dal_widget(&mut self) -> Task<Message> {
        let mostra = self.show_dock(tools::debug::ID, false);
        Task::batch([mostra, self.debug_action(DebugAction::Start)])
    }

    fn esegui_nel_terminale(&mut self, spec: crate::tools::terminal::RunSpec) -> Task<Message> {
        if !self.feature_is_enabled(crate::features::Feature::Terminal) {
            return self
                .notify_error("Esegui: il Terminale è spento nel pannello Estensioni".into());
        }
        let etichetta = spec.label.clone();
        if let Some(tool) = self.registry.get_mut(tools::terminal::ID) {
            let requests = tool.update(&ToolEvent::new(tools::terminal::Event::Run(spec)));
            self.pending_tool_requests.extend(
                requests
                    .into_iter()
                    .map(|request| (tools::terminal::ID, request)),
            );
        }
        let mostra = self.show_dock(tools::terminal::ID, false);
        Task::batch([
            mostra,
            self.drain_tool_requests(),
            self.notify(NoticeKind::Info, format!("Esegui: {etichetta}")),
        ])
    }

    /// Aggiorna il pannello Maven e la sua presenza nella barra.
    pub(super) fn sync_maven_tool(&mut self) {
        let pom = self.setup_corrente().and_then(|setup| setup.pom.clone());
        let comando = self.maven().map(|maven| {
            if maven.wrapper {
                "./mvnw".to_owned()
            } else {
                maven.comando.display().to_string()
            }
        });
        let presente = pom.is_some();
        let requests = self.registry.set_unavailable(tools::maven::ID, !presente);
        self.pending_tool_requests.extend(requests);
        if let Some(tool) = self.registry.get_mut(tools::maven::ID) {
            tool.update(&tools::maven::snapshot_event(tools::maven::MavenView {
                pom,
                comando,
            }));
        }
    }

    /// Un goal dal pannello Maven, con i profili spuntati.
    pub(super) fn esegui_goal_maven(
        &mut self,
        goal: Vec<String>,
        profili: Vec<String>,
    ) -> Task<Message> {
        let Some(root) = self.project_root.clone() else {
            return self.notify_error("Maven: apri prima una cartella di progetto".into());
        };
        let configurazione = crate::configurazioni::Configurazione {
            goal,
            profili,
            ..crate::configurazioni::Configurazione::nuova(
                crate::configurazioni::Tipo::Maven,
                "Maven".into(),
            )
        };
        let maven = self.maven();
        match esecuzione::esegui_configurazione(&configurazione, &root, maven.as_ref()) {
            Ok(mut spec) => {
                spec.label = format!("Maven: {}", configurazione.goal.join(" "));
                self.esegui_nel_terminale(spec)
            }
            Err(problema) => self.notify_error(format!("Maven: {problema}")),
        }
    }

    /// Apre la finestra «Configurazioni di esecuzione» sul file letto.
    pub(super) fn apri_configurazioni(&mut self) -> Task<Message> {
        if self.project_root.is_none() {
            return self.notify_error("Apri prima una cartella di progetto".into());
        }
        let Some(setup) = self.setup_corrente() else {
            let task = self.load_debug_setup();
            return Task::batch([
                task,
                self.notify(
                    NoticeKind::Info,
                    "Lettura delle configurazioni, riprova fra un attimo".into(),
                ),
            ]);
        };
        // Un file che non si legge non si riscrive da qui: la finestra
        // partirebbe vuota e Salva lo cancellerebbe. Si apre come testo.
        let illeggibile = match &setup.configs {
            Err(
                errore @ (lume_dap::LaunchError::Syntax(_)
                | lume_dap::LaunchError::TooLarge
                | lume_dap::LaunchError::Unreadable(_)),
            ) => Some(errore.to_string()),
            _ => None,
        };
        if let Some(errore) = illeggibile {
            let task = self.modifica_configurazioni();
            return Task::batch([
                task,
                self.notify_error(format!("{errore}: aperto come testo")),
            ]);
        }
        let editor = crate::configurazioni_panel::Editor::nuovo(
            &setup.configurazioni,
            setup.maven.is_some(),
        );
        self.configurazioni_editor = Some(editor);
        self.apri_overlay(OverlayId::Configurazioni);
        Task::none()
    }

    pub(super) fn configurazioni_messaggio(
        &mut self,
        messaggio: crate::configurazioni_panel::Messaggio,
    ) -> Task<Message> {
        use crate::configurazioni_panel::Azione;
        let Some(editor) = self.configurazioni_editor.as_mut() else {
            return Task::none();
        };
        match crate::configurazioni_panel::aggiorna(editor, messaggio) {
            Azione::Nessuna => Task::none(),
            Azione::Chiudi => {
                let scartate = editor.modificato;
                self.configurazioni_editor = None;
                self.chiudi_overlay_se(OverlayId::Configurazioni);
                if scartate {
                    self.notify(
                        NoticeKind::Info,
                        "Modifiche alle configurazioni scartate".into(),
                    )
                } else {
                    Task::none()
                }
            }
            Azione::ApriTesto => {
                self.configurazioni_editor = None;
                self.chiudi_overlay_se(OverlayId::Configurazioni);
                self.modifica_configurazioni()
            }
            Azione::Salva(configurazioni) => {
                let Some(root) = self.project_root.clone() else {
                    return Task::none();
                };
                let file = root.join(crate::configurazioni::LAUNCH_FILE);
                let aperto_sporco = self
                    .workspace
                    .get_by_path(&file)
                    .and_then(|id| self.workspace.get(id))
                    .is_some_and(lume_core::Document::is_dirty);
                if aperto_sporco {
                    if let Some(editor) = self.configurazioni_editor.as_mut() {
                        editor.errore = Some(format!(
                            "{} è aperto con modifiche non salvate: salvalo o chiudilo prima",
                            crate::configurazioni::LAUNCH_FILE
                        ));
                    }
                    return Task::none();
                }
                Task::perform(
                    async move {
                        let radice = root.clone();
                        let esito = tokio::task::spawn_blocking(move || {
                            crate::configurazioni::scrivi(&radice, &configurazioni)
                        })
                        .await
                        .unwrap_or_else(|errore| Err(errore.to_string()));
                        (root, esito)
                    },
                    |(root, esito)| Message::ConfigurazioniSalvate(root, esito),
                )
            }
        }
    }

    pub(super) fn configurazioni_salvate(
        &mut self,
        root: PathBuf,
        esito: Result<(), String>,
    ) -> Task<Message> {
        match esito {
            Ok(()) => {
                if self.project_root.as_deref() == Some(root.as_path()) {
                    self.configurazioni_editor = None;
                    self.chiudi_overlay_se(OverlayId::Configurazioni);
                }
                Task::batch([
                    self.load_debug_setup(),
                    self.notify(
                        NoticeKind::Info,
                        format!("{} salvato", crate::configurazioni::LAUNCH_FILE),
                    ),
                ])
            }
            Err(errore) => {
                if let Some(editor) = self.configurazioni_editor.as_mut() {
                    editor.errore = Some(format!("Salvataggio fallito: {errore}"));
                }
                self.notify_error(format!(
                    "{} non salvato: {errore}",
                    crate::configurazioni::LAUNCH_FILE
                ))
            }
        }
    }

    fn modifica_configurazioni(&mut self) -> Task<Message> {
        let Some(root) = self.project_root.clone() else {
            return self.notify_error("Apri prima una cartella di progetto".into());
        };
        let file = root.join(lume_dap::LAUNCH_FILE);
        if file.is_file() {
            return self.open_file_task(file, crate::file_read::DopoApertura::Niente);
        }
        // Gesto esplicito: crea il file solo se non esiste (mai sovrascrive).
        let creato = std::fs::create_dir_all(root.join(".lume")).and_then(|()| {
            use std::io::Write;
            let mut handle = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)?;
            handle.write_all(TEMPLATE_LANCIO.as_bytes())
        });
        match creato {
            Ok(()) => Task::batch([
                self.open_file_task(file, crate::file_read::DopoApertura::Niente),
                self.notify(
                    NoticeKind::Info,
                    format!("Creato {}", lume_dap::LAUNCH_FILE),
                ),
            ]),
            Err(errore) => {
                self.notify_error(format!("Non si crea {}: {errore}", lume_dap::LAUNCH_FILE))
            }
        }
    }
}

/// Compila con `cc`; l'errore riporta le prime righe dello stderr.
fn compila(cc: &Path, argomenti: &[String], binario: &Path) -> Result<(), String> {
    if let Some(cartella) = binario.parent() {
        std::fs::create_dir_all(cartella).map_err(|errore| errore.to_string())?;
    }
    let uscita = std::process::Command::new(cc)
        .args(argomenti)
        .output()
        .map_err(|errore| errore.to_string())?;
    if uscita.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&uscita.stderr);
    Err(stderr.lines().take(4).collect::<Vec<_>>().join(" · "))
}
