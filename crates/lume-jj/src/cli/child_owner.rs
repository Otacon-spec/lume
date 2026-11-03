//! Owner del child diretto. La quarantena conserva, non risolve, l'incertezza.
//! Nessun thread/timer idle e nessun retry dopo un errore di osservazione.
use std::{
    io,
    process::{Child, ExitStatus},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const MAX_OWNERS: usize = 64;
const MAX_DIAGNOSTICS: usize = 64;

pub(super) struct Admission(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl Admission {
    pub(super) fn reserve() -> io::Result<Self> {
        static COUNT: OnceLock<std::sync::Arc<std::sync::atomic::AtomicUsize>> = OnceLock::new();
        Self::reserve_from(
            COUNT
                .get_or_init(|| std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)))
                .clone(),
            MAX_OWNERS,
        )
    }
    fn reserve_from(
        count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        limit: usize,
    ) -> io::Result<Self> {
        use std::sync::atomic::Ordering;
        count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < limit).then_some(n + 1)
            })
            .map_err(|_| {
                io::Error::other("limite owner JJ attivi/incerti raggiunto; raccolta richiesta")
            })?;
        Ok(Self(count))
    }
}
impl Drop for Admission {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

const STOP_WAIT: Duration = Duration::from_secs(2);

// Owner attivi/incerti limitati prima dello spawn; diagnosi limitate separatamente.
// Raccolta esplicita non bloccante; root-close e recovery errori ancora da qualificare.
#[allow(dead_code)]
struct Retained {
    child: Option<Child>,
    _admission: Option<Admission>,
    reason: Box<str>,
    reap_allowed: bool,
}
static RETAINED_CHILDREN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Indicatore atomico senza probe OS, per ammettere una raccolta su worker.
pub fn quarantine_has_children() -> bool {
    RETAINED_CHILDREN.load(std::sync::atomic::Ordering::Acquire) != 0
}
fn retained() -> &'static Mutex<Vec<Retained>> {
    static RETAINED: OnceLock<Mutex<Vec<Retained>>> = OnceLock::new();
    RETAINED.get_or_init(|| Mutex::new(Vec::new()))
}
fn preserve(child: Option<Child>, admission: Option<Admission>, reason: String) {
    preserve_with_policy(child, admission, reason, false);
}
fn preserve_with_policy(
    child: Option<Child>,
    admission: Option<Admission>,
    reason: String,
    reap_allowed: bool,
) {
    let mut records = retained()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let has_child = child.is_some();
    append_retained(&mut records, child, admission, reason, reap_allowed);
    if has_child {
        RETAINED_CHILDREN.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
}
fn append_retained(
    records: &mut Vec<Retained>,
    child: Option<Child>,
    admission: Option<Admission>,
    mut reason: String,
    reap_allowed: bool,
) {
    let mut limit = reason.len().min(2048);
    while !reason.is_char_boundary(limit) {
        limit -= 1;
    }
    reason.truncate(limit);
    if child.is_none()
        && records.iter().filter(|r| r.child.is_none()).count() >= MAX_DIAGNOSTICS
        && let Some(index) = records.iter().position(|r| r.child.is_none())
    {
        records.remove(index);
    }
    // Ogni child possiede la riserva ottenuta prima dello spawn: massimo 64.
    records.push(Retained {
        child,
        _admission: admission,
        reason: reason.into_boxed_str(),
        reap_allowed,
    });
}
/// Esito di un singolo passaggio non bloccante: uncertain non viene riprovato.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JjQuarantineReport {
    pub reaped: usize,
    pub pending: usize,
    pub uncertain: usize,
    pub diagnostics: usize,
}
/// Raccolta esplicita dei child conservati per timeout, senza segnali o attese.
/// Dopo un errore try_wait il child resta posseduto e non viene più osservato.
pub fn collect_quarantine() -> JjQuarantineReport {
    let mut records = retained()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let report = collect_records(&mut records);
    RETAINED_CHILDREN.fetch_sub(report.reaped, std::sync::atomic::Ordering::AcqRel);
    report
}
fn collect_records(records: &mut Vec<Retained>) -> JjQuarantineReport {
    let mut report = JjQuarantineReport::default();
    records.retain_mut(|record| {
        let Some(child) = &mut record.child else {
            report.diagnostics += 1;
            return true;
        };
        if !record.reap_allowed {
            report.uncertain += 1;
            return true;
        }
        match child.try_wait() {
            Ok(Some(_)) => {
                report.reaped += 1;
                false
            }
            Ok(None) => {
                report.pending += 1;
                true
            }
            Err(error) => {
                record.reap_allowed = false;
                record.reason = format!("raccolta esplicita JJ fallita: {error}; nessun retry")
                    .chars()
                    .take(512)
                    .collect::<String>()
                    .into_boxed_str();
                report.uncertain += 1;
                true
            }
        }
    });
    report
}

pub(super) fn record_reader_drop(error: io::Error) {
    preserve(
        None,
        None,
        format!("errore raccolta lettore JJ in Drop: {error}"),
    );
}

pub(super) struct ChildOwner {
    // Some: owned/non-reaped. None: reap confermato oppure trasferimento
    // esplicito in quarantena; l'esito distingue i due casi, non il solo None.
    child: Option<Child>,
    admission: Option<Admission>,
    #[cfg(unix)]
    process_group: Option<u32>,
}
impl ChildOwner {
    pub(super) fn new(child: Child, admission: Admission) -> Self {
        #[cfg(unix)]
        let process_group = Some(child.id());
        Self {
            child: Some(child),
            admission: Some(admission),
            #[cfg(unix)]
            process_group,
        }
    }
    pub(super) fn child(&mut self) -> io::Result<&mut Child> {
        self.child
            .as_mut()
            .ok_or_else(|| io::Error::other("owner del child JJ già raccolto o trasferito"))
    }
    pub(super) fn observe(&mut self) -> io::Result<Option<ExitStatus>> {
        match self.child()?.try_wait() {
            Ok(Some(status)) => {
                // try_wait Some raccoglie il child; mai kill dopo questo punto.
                drop(self.child.take());
                Ok(Some(status))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                let child = self.child.take();
                preserve(
                    child,
                    self.admission.take(),
                    format!("osservazione child JJ fallita: {error}"),
                );
                // Drop vede None: nessun kill o nuovo probe implicito.
                Err(io::Error::other(format!(
                    "{error}; child JJ incerto conservato, nessun kill/retry"
                )))
            }
        }
    }
    pub(super) fn stop(&mut self) -> io::Result<()> {
        if self.child.is_none() {
            return Ok(());
        }
        // Il gruppo viene segnalato mentre il child diretto è ancora posseduto:
        // il suo PID impedisce il riuso del PGID durante il cleanup.
        #[cfg(unix)]
        let kill_error = self.process_group.and_then(|pgid| {
            // SAFETY: negative pid indirizza il gruppo creato per questo child;
            // il child non è stato ancora raccolto, quindi il PGID è ancora
            // ancorato al processo posseduto.
            let result = unsafe { libc::kill(-(pgid as i32), libc::SIGKILL) };
            if result == 0 {
                None
            } else {
                let error = io::Error::last_os_error();
                (error.raw_os_error() != Some(libc::ESRCH)).then_some(error)
            }
        });
        #[cfg(not(unix))]
        let kill_error = self.child()?.kill().err();
        let start = Instant::now();
        loop {
            match self.observe() {
                Ok(Some(_)) => {
                    return match kill_error {
                        None => Ok(()),
                        Some(error) => Err(io::Error::other(format!(
                            "kill JJ fallito: {error}; reap del child confermato"
                        ))),
                    };
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(io::Error::other(match kill_error {
                        None => error.to_string(),
                        Some(kill) => format!("kill JJ fallito: {kill}; {error}"),
                    }));
                }
            }
            if start.elapsed() >= STOP_WAIT {
                let reason = match kill_error {
                    None => "stop JJ: reap non confermato entro il termine".to_owned(),
                    Some(error) => {
                        format!("kill JJ fallito: {error}; reap non confermato entro il termine")
                    }
                };
                let child = self.child.take();
                preserve_with_policy(child, self.admission.take(), reason.clone(), true);
                return Err(io::Error::other(format!(
                    "{reason}; child incerto conservato"
                )));
            }
            std::thread::sleep(super::POLL);
        }
    }
}
impl Drop for ChildOwner {
    fn drop(&mut self) {
        // Fallback worker per unwind/setup incompleto. Non riprovare owner
        // trasferiti; errori non esprimibili nel return restano registrati.
        if self.child.is_some()
            && let Err(error) = self.stop()
        {
            preserve(
                None,
                None,
                format!("cleanup child JJ in Drop non completo: {error}"),
            );
        }
    }
}

#[cfg(test)]
mod admission_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn limite_e_rilascio_slot_sono_isolati() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let a = Admission::reserve_from(count.clone(), 2).unwrap();
        let b = Admission::reserve_from(count.clone(), 2).unwrap();
        assert!(Admission::reserve_from(count.clone(), 2).is_err());
        drop(a);
        let c = Admission::reserve_from(count.clone(), 2).unwrap();
        drop(b);
        drop(c);
        assert_eq!(count.load(std::sync::atomic::Ordering::Acquire), 0);
    }
    #[test]
    fn diagnosi_limitate_e_testo_utf8_limitato() {
        let mut records = Vec::new();
        for _ in 0..100 {
            append_retained(&mut records, None, None, "😀".repeat(1024), false);
        }
        assert_eq!(records.len(), MAX_DIAGNOSTICS);
        assert!(records.iter().all(|r| r.reason.len() <= 2048));
    }
    #[cfg(unix)]
    #[test]
    fn raccolta_child_reale_rilascia_slot_solo_dopo_exit() {
        use std::io::Write;
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let admission = Admission::reserve_from(count.clone(), 1).unwrap();
        let mut child =
            super::super::spawn_owner_fixture("read value", std::process::Stdio::piped()).unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut records = Vec::new();
        append_retained(
            &mut records,
            Some(child),
            Some(admission),
            "timeout".into(),
            true,
        );
        let report = collect_records(&mut records);
        assert_eq!(report.pending, 1);
        assert_eq!(report.reaped, 0);
        assert!(Admission::reserve_from(count.clone(), 1).is_err());
        input.write_all(b"ok\n").unwrap();
        drop(input);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !records.is_empty() && Instant::now() < deadline {
            collect_records(&mut records);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(records.is_empty());
        assert_eq!(count.load(std::sync::atomic::Ordering::Acquire), 0);
    }
    #[cfg(unix)]
    #[test]
    fn errore_wait_reale_rimane_incerto_senza_secondo_probe() {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let admission = Admission::reserve_from(count.clone(), 1).unwrap();
        let child =
            super::super::spawn_owner_fixture("exit 0", std::process::Stdio::null()).unwrap();
        let pid = child.id() as i32;
        let mut status = 0;
        // SAFETY: PID esatto del child fixture posseduto, status puntatore valido.
        // Simula un reaper esterno; non segnala o raccoglie altri processi.
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        let mut records = Vec::new();
        append_retained(
            &mut records,
            Some(child),
            Some(admission),
            "timeout".into(),
            true,
        );
        let report = collect_records(&mut records);
        assert_eq!(report.uncertain, 1);
        assert_eq!(report.reaped, 0);
        assert!(records.iter().all(|r| !r.reap_allowed));
        assert_eq!(collect_records(&mut records).uncertain, 1);
        assert!(Admission::reserve_from(count, 1).is_err());
        // Il processo fixture è già raccolto da waitpid; solo cleanup del test.
    }
}
