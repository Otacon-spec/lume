//! Il cancello delle azioni MCP che modificano o eseguono.
//!
//! `decide` è una tabella pura: nessuna modalità può autorizzare una scrittura
//! fuori dalla radice o sotto `.git`. Chi invoca la tabella deve verificare il
//! bersaglio con il confinamento filesystem, non solo con il percorso fornito.

use std::path::{Component, Path};

/// Modalità di accesso del progetto. Per default si applica la più cauta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    /// Ogni tool che modifica o esegue chiede conferma nell'IDE.
    #[default]
    NonBrave,
    /// Le modifiche interne sono automatiche; l'esecuzione chiede conferma.
    Auto,
    /// Nessuna conferma per i bersagli consentiti.
    Brave,
}

impl Mode {
    pub const ALL: [Self; 3] = [Self::NonBrave, Self::Auto, Self::Brave];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NonBrave => "non_brave",
            Self::Auto => "auto",
            Self::Brave => "brave",
        }
    }

    pub fn from_str_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == name)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::NonBrave => "Non brave",
            Self::Auto => "Auto",
            Self::Brave => "Brave",
        }
    }

    pub const fn index(self) -> u8 {
        match self {
            Self::NonBrave => 0,
            Self::Auto => 1,
            Self::Brave => 2,
        }
    }

    /// Un indice non riconosciuto ricade sulla modalità più cauta.
    pub const fn from_index(index: u8) -> Self {
        match index {
            1 => Self::Auto,
            2 => Self::Brave,
            _ => Self::NonBrave,
        }
    }
}

/// Effetto del tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Effect {
    Read,
    Modify,
    Execute,
}

/// Bersaglio rispetto alla radice del progetto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Inside,
    Outside,
    GitDir,
}

impl Target {
    /// Classifica un percorso relativo. Il controllo filesystem autorevole è
    /// `Root::resolve_for_write`; questa funzione è una seconda barriera.
    pub fn of_relative(relative: &Path) -> Self {
        let mut in_git = false;
        for component in relative.components() {
            match component {
                Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                    return Self::Outside;
                }
                Component::Normal(name) => {
                    if is_git_name(&name.to_string_lossy()) {
                        in_git = true;
                    }
                }
                Component::CurDir => {}
            }
        }
        if in_git { Self::GitDir } else { Self::Inside }
    }
}

/// Motivo per cui l'azione viene rifiutata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Refusal {
    Outside,
    GitDir,
}

impl Refusal {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Outside => {
                "vietato: l'azione esce dalla radice del progetto (in nessuna modalità)"
            }
            Self::GitDir => "vietato: la cartella .git non si modifica (in nessuna modalità)",
        }
    }
}

/// Esito del cancello.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    Allow,
    Ask,
    Refuse(Refusal),
}

/// Regole di accesso registrate nella decisione utente del 29/09.
pub fn decide(mode: Mode, effect: Effect, target: Target) -> Verdict {
    match target {
        Target::Outside => return Verdict::Refuse(Refusal::Outside),
        Target::GitDir => return Verdict::Refuse(Refusal::GitDir),
        Target::Inside => {}
    }
    match (mode, effect) {
        (_, Effect::Read) => Verdict::Allow,
        (Mode::NonBrave, Effect::Modify) => Verdict::Ask,
        (Mode::Auto | Mode::Brave, Effect::Modify) => Verdict::Allow,
        (Mode::Brave, Effect::Execute) => Verdict::Allow,
        (Mode::NonBrave | Mode::Auto, Effect::Execute) => Verdict::Ask,
    }
}

/// Riconosce `.git` anche nelle forme che NTFS può risolvere allo stesso
/// componente: maiuscole, punti/spazi finali, ADS e alias 8.3 `GIT~N`.
pub fn is_git_name(name: &str) -> bool {
    let base = name.split(':').next().unwrap_or(name);
    let base = base.trim_end_matches(['.', ' ']);
    if base.eq_ignore_ascii_case(".git") {
        return true;
    }
    base.to_ascii_lowercase()
        .strip_prefix("git~")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::path::Path;

    #[test]
    fn la_tabella_completa_di_decide() {
        use Effect::{Execute, Modify, Read};
        use Mode::{Auto, Brave, NonBrave};
        use Verdict::{Allow, Ask};
        let dentro = [
            (NonBrave, Read, Allow),
            (NonBrave, Modify, Ask),
            (NonBrave, Execute, Ask),
            (Auto, Read, Allow),
            (Auto, Modify, Allow),
            (Auto, Execute, Ask),
            (Brave, Read, Allow),
            (Brave, Modify, Allow),
            (Brave, Execute, Allow),
        ];
        for (mode, effect, expected) in dentro {
            assert_eq!(decide(mode, effect, Target::Inside), expected);
        }
    }

    #[test]
    fn fuori_radice_e_sotto_git_si_rifiuta_sempre() {
        for mode in Mode::ALL {
            for effect in [Effect::Read, Effect::Modify, Effect::Execute] {
                assert_eq!(
                    decide(mode, effect, Target::Outside),
                    Verdict::Refuse(Refusal::Outside)
                );
                assert_eq!(
                    decide(mode, effect, Target::GitDir),
                    Verdict::Refuse(Refusal::GitDir)
                );
            }
        }
    }

    #[test]
    fn il_bersaglio_si_legge_dal_percorso_relativo() {
        assert_eq!(
            Target::of_relative(Path::new("src/main.rs")),
            Target::Inside
        );
        assert_eq!(Target::of_relative(Path::new("")), Target::Inside);
        assert_eq!(Target::of_relative(Path::new(".gitignore")), Target::Inside);
        assert_eq!(
            Target::of_relative(Path::new("src/.github/x.yml")),
            Target::Inside
        );
        assert_eq!(
            Target::of_relative(Path::new(".git/config")),
            Target::GitDir
        );
        assert_eq!(
            Target::of_relative(Path::new("sub/.GIT/hooks/pre-commit")),
            Target::GitDir
        );
        assert_eq!(
            Target::of_relative(Path::new("../fuori.txt")),
            Target::Outside
        );
        assert_eq!(
            Target::of_relative(Path::new("a/../../fuori.txt")),
            Target::Outside
        );
        assert_eq!(
            Target::of_relative(Path::new("../.git/config")),
            Target::Outside
        );
    }

    #[test]
    fn i_nomi_che_windows_legge_come_git_sono_git() {
        for name in [
            ".git",
            ".GIT",
            ".Git",
            ".git.",
            ".git ",
            ".git. .",
            "GIT~1",
            "git~2",
            ".git::$INDEX_ALLOCATION",
            ".git:stream",
        ] {
            assert!(is_git_name(name), "{name:?}");
        }
        for name in [
            ".gitignore",
            ".github",
            "git",
            "digit",
            "git~",
            "git~x",
            ".gitmodules",
            "a.git",
            "",
        ] {
            assert!(!is_git_name(name), "{name:?}");
        }
    }

    #[test]
    fn la_modalita_si_riconosce_dal_nome_e_dall_indice() {
        assert_eq!(Mode::default(), Mode::NonBrave);
        for mode in Mode::ALL {
            assert_eq!(Mode::from_str_name(mode.as_str()), Some(mode));
            assert_eq!(Mode::from_index(mode.index()), mode);
        }
        assert_eq!(Mode::from_str_name("BRAVE"), None);
        assert_eq!(Mode::from_str_name(""), None);
        assert_eq!(Mode::from_index(3), Mode::NonBrave);
        assert_eq!(Mode::from_index(255), Mode::NonBrave);
        assert_eq!(Mode::NonBrave.label(), "Non brave");
        assert_eq!(Mode::Auto.label(), "Auto");
        assert_eq!(Mode::Brave.label(), "Brave");
    }

    #[test]
    fn i_rifiuti_hanno_un_messaggio_in_italiano() {
        assert!(Refusal::Outside.message().contains("radice"));
        assert!(Refusal::GitDir.message().contains(".git"));
    }
}
