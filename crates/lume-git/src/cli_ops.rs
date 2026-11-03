//! Operazioni dell'eccezione D3, backend reale; nessun lancio da inattivo.
//! Nomi validati e ref completi: nessun nome diventa opzione o DWIM.
use crate::{GitCli, GitError, GitRepository};
use std::sync::atomic::AtomicBool;

pub fn validate_ref_name(name: &str) -> Result<(), GitError> {
    // Il valore arriva anche dalla configurazione: potrebbe essere un URL
    // con credenziali inserito nel campo sbagliato. Non riportarlo nell'errore.
    let invalid = |why: &str| GitError::InvalidBranchName(why.to_owned());
    if name.is_empty() || name.starts_with('-') {
        return Err(invalid("è vuoto o comincia con un'opzione"));
    }
    if name
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c))
    {
        return Err(invalid("contiene spazi o caratteri non ammessi"));
    }
    if name.contains("..") || name.contains("@{") || name.contains("//") {
        return Err(invalid("contiene una sequenza non ammessa"));
    }
    if name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name
            .split('/')
            .any(|p| p.starts_with('.') || p.ends_with(".lock"))
    {
        return Err(invalid("contiene un componente non ammesso"));
    }
    Ok(())
}
impl GitRepository {
    /// Solo branch locali già esistenti: --no-guess non crea da un remoto.
    /// Git preserva le modifiche che non può portare senza sovrascriverle.
    pub fn switch_branch(
        &self,
        cli: &GitCli,
        name: &str,
        cancel: &AtomicBool,
    ) -> Result<(), GitError> {
        validate_ref_name(name)?;
        let out = cli.run(
            &self.work_dir,
            &["switch", "--no-guess", "--", name],
            cancel,
        )?;
        if out.success() {
            Ok(())
        } else {
            Err(GitError::Switch(out.failure_text()))
        }
    }
    /// Crea un locale esplicitamente dal ref remoto completo; non una abbreviazione
    /// ambigua se esiste anche un branch locale origin/nome.
    pub fn switch_remote_branch(
        &self,
        cli: &GitCli,
        remote: &str,
        name: &str,
        cancel: &AtomicBool,
    ) -> Result<String, GitError> {
        validate_ref_name(remote)?;
        validate_ref_name(name)?;
        let start = format!("refs/remotes/{remote}/{name}");
        let out = cli.run(
            &self.work_dir,
            &["switch", "--track", "--create", name, "--", &start],
            cancel,
        )?;
        if out.success() {
            Ok(name.to_owned())
        } else {
            Err(GitError::Switch(out.failure_text()))
        }
    }
    /// Destinazione upstream esplicita; fallback origin solo con configurazione
    /// upstream assente. Non forza, non pubblica tag/altri branch implicitamente
    /// e non avvia push ricorsivi dei sottomoduli dalla configurazione globale.
    pub fn push(&self, cli: &GitCli, cancel: &AtomicBool) -> Result<String, GitError> {
        let branch = self
            .branch_name()?
            .ok_or_else(|| GitError::Push("HEAD è staccato: nessun branch da pubblicare".into()))?;
        validate_ref_name(&branch)?;
        let (remote, target, set_upstream) = match self.upstream()? {
            Some(up) => (up.remote, up.branch, false),
            None if self
                .config_string(&format!("branch.{branch}.remote"))
                .is_some() =>
            {
                return Err(GitError::NoUpstream(branch));
            }
            None if self.has_origin() => ("origin".into(), branch.clone(), true),
            None => return Err(GitError::NoUpstream(branch)),
        };
        validate_ref_name(&remote)?;
        validate_ref_name(&target)?;
        // Non HEAD: un cambio esterno di branch non deve cambiare la sorgente
        // fra la lettura del branch corrente e l'avvio della CLI.
        let refspec = format!("refs/heads/{branch}:refs/heads/{target}");
        let mut args = vec![
            "push",
            "--no-force",
            "--no-mirror",
            "--no-follow-tags",
            "--recurse-submodules=no",
        ];
        if set_upstream {
            args.push("--set-upstream");
        }
        args.extend(["--", &remote, &refspec]);
        // Git rilegge remote.<nome>.mirror dopo il parsing delle opzioni:
        // --no-mirror da solo non prevale. Override solo del figlio, non
        // persistente, senza reinterpretare '=' nel nome come parte del valore.
        let mirror = format!("remote.{remote}.mirror");
        let out = cli.run_with_config(&self.work_dir, &args, cancel, Some((&mirror, "false")))?;
        if out.success() {
            Ok(format!("«{branch}» pubblicato su {remote}/{target}"))
        } else {
            Err(GitError::Push(out.failure_text()))
        }
    }
}
