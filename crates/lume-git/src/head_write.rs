//! Snapshot della catena di HEAD, validata dopo prepare dei lock nativi gix.
//! Stesso OID su un altro branch non riapplica il piano. Non è confinamento
//! filesystem né transazione index/worktree; oggetti ODB già scritti restano.
use crate::GitError;
use gix::refs::{
    FullName, Target,
    transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog},
};
use std::collections::BTreeSet;

pub(crate) struct HeadSnapshot {
    chain: Vec<(FullName, Option<Target>)>,
    pub(crate) id: Option<gix::ObjectId>,
}
/// Owner non clonabile: la verifica no-op non espone la transazione Delete
/// preparata, che NON deve mai essere committata. Drop libera i marker nativi.
pub(crate) struct PreparedHead<'r> {
    transaction: gix::refs::file::Transaction<'r, 'r>,
    publish: bool,
}
impl PreparedHead<'_> {
    pub(crate) fn commit(
        self,
        committer: Option<gix::actor::SignatureRef<'_>>,
    ) -> Result<(), GitError> {
        if self.publish {
            self.transaction.commit(committer).map_err(|error| {
                GitError::Commit(format!(
                    "Aggiornamento ref/reflog non concluso; esito repository da verificare: {error}"
                ))
            })?;
        } else {
            // Soltanto verifica: nessun ref/reflog cancellato o riapplicato.
            drop(self.transaction);
        }
        Ok(())
    }
}
impl HeadSnapshot {
    pub(crate) fn capture(repo: &gix::Repository) -> Result<Self, GitError> {
        let mut name: FullName = "HEAD"
            .try_into()
            .map_err(|_| GitError::Commit("Nome HEAD non valido".into()))?;
        let mut seen = BTreeSet::new();
        let mut chain = Vec::new();
        let id = loop {
            if !seen.insert(name.as_bstr().to_owned()) {
                return Err(GitError::Commit("Catena HEAD ciclica".into()));
            }
            let target = match repo.find_reference(&name) {
                Ok(reference) => Some(reference.target().into_owned()),
                Err(error) if error.is_not_found() && !chain.is_empty() => None,
                Err(error) => return Err(GitError::Commit(error.to_string())),
            };
            chain.push((name.clone(), target.clone()));
            match target {
                Some(Target::Symbolic(next)) => name = next,
                Some(Target::Object(id)) => break Some(id),
                None => break None,
            }
        };
        Ok(Self { chain, id })
    }
    pub(crate) fn ensure_current(&self, repo: &gix::Repository) -> Result<(), GitError> {
        if Self::capture(repo)?.chain == self.chain {
            Ok(())
        } else {
            Err(GitError::Commit(
                "HEAD/branch è cambiato durante il fetch; integrazione annullata".into(),
            ))
        }
    }
    pub(crate) fn same_chain(&self, other: &Self) -> bool {
        self.chain == other.chain
    }
    pub(crate) fn prepare<'r>(
        &self,
        repo: &'r gix::Repository,
        to: gix::ObjectId,
        message: impl Into<gix::bstr::BString>,
    ) -> Result<PreparedHead<'r>, GitError> {
        let publish = self.id != Some(to);
        let expected = self.id.map_or(PreviousValue::MustNotExist, |id| {
            PreviousValue::MustExistAndMatch(Target::Object(id))
        });
        let edits = if publish {
            vec![RefEdit {
                name: "HEAD"
                    .try_into()
                    .map_err(|_| GitError::Commit("Nome HEAD non valido".into()))?,
                deref: true,
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: message.into(),
                    },
                    expected,
                    new: Target::Object(to),
                },
            }]
        } else {
            // Update identico ottimizza via il lock leaf. Delete/Only prepara
            // marker per ogni target RAW, senza deref: owner impedisce commit.
            self.chain
                .iter()
                .map(|(name, target)| {
                    let target = target.clone().ok_or_else(|| {
                        GitError::Commit("Verifica no-op senza leaf esistente".into())
                    })?;
                    Ok(RefEdit {
                        name: name.clone(),
                        deref: false,
                        change: Change::Delete {
                            expected: PreviousValue::MustExistAndMatch(target),
                            log: RefLog::Only,
                        },
                    })
                })
                .collect::<Result<Vec<_>, GitError>>()?
        };
        let transaction = repo
            .refs
            .transaction()
            .prepare(
                edits,
                gix::lock::acquire::Fail::Immediately,
                gix::lock::acquire::Fail::Immediately,
            )
            .map_err(|error| GitError::Commit(error.to_string()))?;
        // prepare ha già acquisito HEAD, ogni symref intermedio e il leaf.
        // OID uguale non basta: ricontrollare la catena ora sotto quei lock.
        if Self::capture(repo)?.chain != self.chain {
            return Err(GitError::Commit(
                "HEAD/branch cambiato: piano precedente invalidato prima di aggiornare ref/reflog"
                    .into(),
            ));
        }
        Ok(PreparedHead {
            transaction,
            publish,
        })
    }
    pub(crate) fn advance(
        &self,
        repo: &gix::Repository,
        to: gix::ObjectId,
        message: impl Into<gix::bstr::BString>,
    ) -> Result<(), GitError> {
        let transaction = self.prepare(repo, to, message)?;
        let committer = repo
            .committer()
            .transpose()
            .map_err(|error| GitError::Commit(error.to_string()))?;
        transaction.commit(committer)
    }
}
#[cfg(test)]
mod tests;
