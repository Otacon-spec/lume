//! Upstream esplicito, senza indovinare la destinazione di un push/pull.
use crate::{GitError, GitRepository, validate_ref_name};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Upstream {
    pub remote: String,
    /// Nome sul remote, anche feature/x.
    pub branch: String,
    /// Solo refspec standard: refs/remotes/<remote>/<branch>.
    pub tracking_ref: String,
}
impl GitRepository {
    pub(crate) fn config_string(&self, key: &str) -> Option<String> {
        self.repository
            .config_snapshot()
            .string(key)
            .map(|v| v.to_string())
    }
    /// None solo se assente/detached o upstream locale (non remoto).
    /// Configurazione parziale/refspec personalizzato: errore, mai ripiego
    /// silenzioso verso origin e mai un tracking_ref inventato (G5).
    pub fn upstream(&self) -> Result<Option<Upstream>, GitError> {
        let Some(branch) = self.branch_name()? else {
            return Ok(None);
        };
        let remote = self.config_string(&format!("branch.{branch}.remote"));
        let merge = self.config_string(&format!("branch.{branch}.merge"));
        let (remote, merge) = match (remote, merge) {
            (None, None) => return Ok(None),
            (Some(r), Some(m)) => (r, m),
            _ => return Err(GitError::NoUpstream(branch)),
        };
        if remote == "." {
            return Ok(None);
        }
        // Git consente più merge ref (octopus). Questo backend promette
        // un solo upstream: non selezionare silenziosamente l'ultimo.
        if self
            .repository
            .config_snapshot()
            .strings(format!("branch.{branch}.merge").as_str())
            .is_none_or(|s| s.len() != 1)
        {
            return Err(GitError::NoUpstream(branch));
        }
        validate_ref_name(&remote)?;
        let short = merge
            .strip_prefix("refs/heads/")
            .ok_or_else(|| GitError::NoUpstream(branch.clone()))?;
        validate_ref_name(short)?;
        let standard = format!("+refs/heads/*:refs/remotes/{remote}/*");
        let specs = self
            .repository
            .config_snapshot()
            .strings(format!("remote.{remote}.fetch").as_str());
        if !specs.is_some_and(|s| {
            s.len() == 1
                && s.first()
                    .is_some_and(|v| v.as_slice() == standard.as_bytes())
        }) {
            return Err(GitError::NoUpstream(branch));
        }
        Ok(Some(Upstream {
            tracking_ref: format!("refs/remotes/{remote}/{short}"),
            branch: short.to_owned(),
            remote,
        }))
    }
}
