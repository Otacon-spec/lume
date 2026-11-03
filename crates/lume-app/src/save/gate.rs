//! Autorità consumante per una scrittura cooperativa. Nessun handle filesystem né lease
//! forte nel gate: l'ultimo consumer perso nega e risveglia il worker.
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Preparing,
    Ready,
    Granted,
    CommitStarted,
    Aborting,
    Done,
}
struct Flags {
    state: State,
    ui_busy: usize,
}
pub(crate) struct Gate {
    flags: Mutex<Flags>,
    wake: Condvar,
    #[cfg(test)]
    commit_started_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
impl Gate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            flags: Mutex::new(Flags {
                state: State::Preparing,
                ui_busy: 0,
            }),
            wake: Condvar::new(),
            #[cfg(test)]
            commit_started_hook: Mutex::new(None),
        })
    }
    fn lock(&self) -> MutexGuard<'_, Flags> {
        self.flags
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub(crate) fn ready(self: &Arc<Self>) -> Option<Lease> {
        let mut flags = self.lock();
        if flags.state != State::Preparing {
            return None;
        }
        flags.state = State::Ready;
        Some(Lease(Arc::new(Decision { gate: self.clone() })))
    }
    pub(crate) fn is_ready(&self) -> bool {
        self.lock().state == State::Ready
    }
    pub(crate) fn deny(&self) {
        {
            let mut flags = self.lock();
            if matches!(
                flags.state,
                State::Preparing | State::Ready | State::Granted
            ) {
                flags.state = State::Aborting;
            }
        }
        self.wake.notify_all();
    }
    /// Il permesso consente solo la verifica finale del preparato: resta
    /// revocabile mentre il worker controlla gli alias, senza pubblicare.
    pub(crate) fn wait_permission(&self) -> bool {
        let mut flags = self.lock();
        loop {
            match flags.state {
                State::Granted => return true,
                State::Aborting | State::Done => return false,
                _ => {
                    flags = self
                        .wake
                        .wait(flags)
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                }
            }
        }
    }
    /// Predicato e registrazione Condvar sotto lo stesso lock. Il confine è
    /// monotono: una revoca successiva raccoglie il commit, non fa rollback.
    pub(crate) fn wait_commit(&self) -> bool {
        let started = {
            let mut flags = self.lock();
            loop {
                match flags.state {
                    State::Granted if flags.ui_busy == 0 => {
                        flags.state = State::CommitStarted;
                        break true;
                    }
                    State::Aborting | State::Done => break false,
                    _ => {
                        flags = self
                            .wake
                            .wait(flags)
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                    }
                }
            }
        };
        #[cfg(test)]
        if started
            && let Some(hook) = self
                .commit_started_hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        {
            hook();
        }
        started
    }
    #[cfg(test)]
    pub(crate) fn set_commit_started_hook(&self, hook: impl FnOnce() + Send + 'static) {
        *self
            .commit_started_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(hook));
    }
    pub(crate) fn done(&self) {
        {
            self.lock().state = State::Done;
        }
        self.wake.notify_all();
    }
    pub(crate) fn pause(self: &Arc<Self>) -> Pause {
        self.lock().ui_busy += 1;
        Pause {
            gate: self.clone(),
            finished: false,
        }
    }
}
struct Decision {
    gate: Arc<Gate>,
}
impl Drop for Decision {
    fn drop(&mut self) {
        self.gate.deny();
    }
}
#[derive(Clone)]
pub(crate) struct Lease(Arc<Decision>);
pub(crate) struct WeakLease(std::sync::Weak<Decision>);
impl Lease {
    pub(crate) fn downgrade(&self) -> WeakLease {
        WeakLease(Arc::downgrade(&self.0))
    }
    pub(crate) fn grant(&self) -> bool {
        let granted = {
            let mut flags = self.0.gate.lock();
            if flags.state != State::Ready {
                return false;
            }
            flags.state = State::Granted;
            true
        };
        self.0.gate.wake.notify_all();
        granted
    }
    pub(crate) fn deny(&self) {
        self.0.gate.deny();
    }
}
impl WeakLease {
    pub(crate) fn upgrade(&self) -> Option<Lease> {
        self.0.upgrade().map(Lease)
    }
    pub(crate) fn matches(&self, lease: &Lease) -> bool {
        std::sync::Weak::ptr_eq(&self.0, &Arc::downgrade(&lease.0))
    }
}
/// Non trattiene l'autorità decisionale. Il Drop per panic/uscita incompleta
/// nega; nessuna callback e nessun I/O sotto il mutex del protocollo.
pub(crate) struct Pause {
    gate: Arc<Gate>,
    finished: bool,
}
impl Pause {
    pub(crate) fn finish(mut self, valid: bool) {
        if !valid {
            self.gate.deny();
        }
        self.finished = true;
    }
}
impl Drop for Pause {
    fn drop(&mut self) {
        if !self.finished {
            self.gate.deny();
        }
        {
            self.gate.lock().ui_busy -= 1;
        }
        self.gate.wake.notify_all();
    }
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
