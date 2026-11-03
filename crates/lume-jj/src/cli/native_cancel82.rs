//! JJ reale bloccato dal suo flock working-copy; non fake process/editor/helper.
//! Fixture di scheduling controllata, non GUI/process-tree/native fault generale.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    sync::{Arc, Mutex, OnceLock},
    thread::JoinHandle,
    time::Instant,
};

type Job82 = JoinHandle<Result<JjOutput, JjError>>;
type Esito82<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn workers82() -> io::Result<usize> {
    let mut n = 0;
    for entry in std::fs::read_dir("/proc/self/task")? {
        match std::fs::read_to_string(entry?.path().join("comm")) {
            Ok(name) if name.trim() == "lume-jj-pipe" => n += 1,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(n)
}
fn children82() -> io::Result<HashSet<u32>> {
    let mut pids = HashSet::new();
    for entry in std::fs::read_dir("/proc/self/task")? {
        match std::fs::read_to_string(entry?.path().join("children")) {
            Ok(text) => {
                for pid in text.split_whitespace() {
                    pids.insert(pid.parse().map_err(io::Error::other)?);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(pids)
}
fn identity82(pid: u32) -> io::Result<Option<String>> {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => {
            // Dopo l'ultima ')' field3; starttime è field22. Non parsing del
            // nome/argomenti né azioni su PID ricostruiti.
            let (_, rest) = text
                .rsplit_once(')')
                .ok_or_else(|| io::Error::other("stat processo owned senza delimitatore"))?;
            Ok(Some(
                rest.split_whitespace()
                    .nth(19)
                    .ok_or_else(|| io::Error::other("starttime processo owned assente"))?
                    .to_owned(),
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
fn waiting82(file: &File, program: &Path) -> io::Result<Option<(u32, String)>> {
    let metadata = file.metadata()?;
    let dev = metadata.dev();
    let children = children82()?;
    // Non registrare /proc/locks grezzo: selezionare solo l'inode posseduto e
    // il child di questo processo col vero exe atteso.
    for line in std::fs::read_to_string("/proc/locks")?.lines() {
        let mut tokens = line.split_whitespace();
        if tokens.next().is_none()
            || tokens.next() != Some("->")
            || tokens.next() != Some("FLOCK")
            || tokens.next() != Some("ADVISORY")
            || tokens.next() != Some("WRITE")
        {
            continue;
        }
        let Some(pid) = tokens.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(device) = tokens.next() else {
            continue;
        };
        let mut address = device.split(':');
        let major = address.next().and_then(|s| u32::from_str_radix(s, 16).ok());
        let minor = address.next().and_then(|s| u32::from_str_radix(s, 16).ok());
        let inode = address.next().and_then(|s| s.parse::<u64>().ok());
        if major != Some(libc::major(dev))
            || minor != Some(libc::minor(dev))
            || inode != Some(metadata.ino())
            || !children.contains(&pid)
        {
            continue;
        }
        match std::fs::read_link(format!("/proc/{pid}/exe")) {
            Ok(exe) if exe == program => {
                if let Some(start) = identity82(pid)? {
                    return Ok(Some((pid, start)));
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}
fn flock82(file: &File, operation: i32) -> io::Result<()> {
    // SAFETY: fd della File viva/posseduta dalla fixture; nessun fd arbitrario.
    if unsafe { libc::flock(file.as_raw_fd(), operation) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn ready82(job: &Option<Job82>, budget: Duration) -> bool {
    let start = Instant::now();
    while job.as_ref().is_some_and(|j| !j.is_finished()) && start.elapsed() < budget {
        std::thread::sleep(Duration::from_millis(5));
    }
    job.as_ref().is_none_or(|j| j.is_finished())
}
fn retain82(job: Job82) {
    // Solo fallback test incerto: owner persistente, niente detach/nuovo worker
    // o polling idle. Non qualifica recovery/resource bounds/process exit.
    static UNCERTAIN: OnceLock<Mutex<Vec<Job82>>> = OnceLock::new();
    UNCERTAIN
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(job);
}

fn marker_running82(pid: u32, marker: &str) -> io::Result<bool> {
    match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(command_line) => Ok(command_line
            .windows(marker.len())
            .any(|part| part == marker.as_bytes())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

struct DescendantCleanup82 {
    pid_file: PathBuf,
    marker: String,
}

impl Drop for DescendantCleanup82 {
    fn drop(&mut self) {
        let Ok(pid) = std::fs::read_to_string(&self.pid_file) else {
            return;
        };
        let Ok(pid) = pid.trim().parse::<u32>() else {
            return;
        };
        if marker_running82(pid, &self.marker).unwrap_or(false) {
            // SAFETY: test fixture identifies the process by a unique argv
            // marker immediately before signaling its observed PID.
            let _ = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
    }
}

fn quote_sh82(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[test]
fn cancel_kills_descendants_in_owned_process_group() -> Esito82 {
    use std::os::unix::fs::PermissionsExt;

    let fixture = tempfile::tempdir()?;
    let pid_file = fixture.path().join("descendant.pid");
    let marker = format!("lume-jj-descendant-{}", std::process::id());
    let executable = fixture.path().join("jj-fixture");
    let script = format!(
        "#!/bin/sh\nshift 3\n/usr/bin/python3 -c 'import time; time.sleep(60)' {} &\nchild=$!\nprintf '%s\\n' \"$child\" > {}\nwait \"$child\"\n",
        quote_sh82(&marker),
        quote_sh82(&pid_file.to_string_lossy())
    );
    std::fs::write(&executable, script)?;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;

    let cleanup = DescendantCleanup82 {
        pid_file: pid_file.clone(),
        marker: marker.clone(),
    };
    let cli = JjCli {
        program: executable,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let run_cancel = Arc::clone(&cancel);
    let root = fixture.path().to_path_buf();
    let job = std::thread::spawn(move || cli.run(&root, &[], &run_cancel));
    let start = Instant::now();
    let pid = loop {
        if let Ok(text) = std::fs::read_to_string(&pid_file) {
            break text.trim().parse::<u32>()?;
        }
        if job.is_finished() || start.elapsed() > Duration::from_secs(5) {
            return Err(io::Error::other("discendente JJ fixture non avviato").into());
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let deadline = start + Duration::from_secs(5);
    loop {
        if marker_running82(pid, &marker)? {
            break;
        }
        if job.is_finished() || Instant::now() >= deadline {
            return Err(io::Error::other("discendente fixture non è diventato attivo").into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    cancel.store(true, Ordering::Release);
    let result = job
        .join()
        .map_err(|_| io::Error::other("worker JJ fixture terminato con panic"))?;
    assert!(matches!(result, Err(JjError::Cancelled)));

    let deadline = Instant::now() + Duration::from_secs(2);
    while marker_running82(pid, &marker)? && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !marker_running82(pid, &marker)?,
        "annullamento ha raccolto il figlio diretto ma lasciato vivo il discendente"
    );
    drop(cleanup);
    Ok(())
}

#[test]
#[ignore = "JJ cancel reale Linux opt-in: test-jj-cancel-real"]
fn jj_reale_attesa_flock_cancel_raccolta_successore() -> Esito82 {
    let program = std::env::var_os("LUME_JJ")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("jj reale esplicito mancante"))?;
    if !program.is_absolute() || !program.is_file() {
        return Err(io::Error::other("jj reale non assoluto/file").into());
    }
    let cli = JjCli::locate().ok_or_else(|| io::Error::other("discovery jj produzione assente"))?;
    assert_eq!(cli.program(), program);
    let home = std::env::var_os("LUME_JJ_TEST_HOME")
        .ok_or_else(|| io::Error::other("profilo privato runner mancante"))?;
    assert_eq!(std::env::var_os("HOME"), Some(home));
    let fixture = tempfile::Builder::new()
        .prefix("lume-jj82-città spazio-")
        .tempdir()?;
    let root = fixture.path().to_path_buf();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut lock = None;
    let mut job = None; // fuori dal catch: nessun owner perso durante panic.
    let scenario = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Esito82 {
        assert_eq!(workers82()?, 0);
        assert!(cli.run(&root, &["git", "init"], &cancel)?.success());
        let text = root.join("città.txt");
        std::fs::write(&text, "prima 😀\n")?;
        let path = root.join(".jj/working_copy/working_copy.lock");
        let held = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        // CLOEXEC evita che la fixture stessa doni la lease al child.
        // SAFETY: `held` resta aperto per la chiamata e `fcntl` legge i flag del suo fd valido.
        let flags = unsafe { libc::fcntl(held.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
        flock82(&held, libc::LOCK_EX | libc::LOCK_NB)?;
        lock = Some(held);
        let native = cli.clone();
        let directory = root.clone();
        let flag = cancel.clone();
        job = Some(
            std::thread::Builder::new()
                .name("lume-jj82-run".into())
                .spawn(move || native.run(&directory, &["status"], &flag))?,
        );
        let start = Instant::now();
        let origin = loop {
            if let Some(origin) = waiting82(lock.as_ref().unwrap(), &program)? {
                break origin;
            }
            if job.as_ref().unwrap().is_finished() || start.elapsed() > Duration::from_secs(10) {
                return Err(io::Error::other(
                    "nessun autentico child JJ in attesa del flock fixture",
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let reader_start = Instant::now();
        while workers82()? != 2 {
            if job.as_ref().unwrap().is_finished()
                || reader_start.elapsed() > Duration::from_secs(10)
            {
                return Err(io::Error::other(
                    "entrambi i reader nativi non osservati durante l'attesa flock",
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        cancel.store(true, Ordering::Release);
        assert!(
            ready82(&job, Duration::from_secs(10)),
            "raccolta job prima di liberare il lock"
        );
        let result = job
            .take()
            .unwrap()
            .join()
            .map_err(|_| io::Error::other("worker JJ82 panic"))?;
        assert!(
            matches!(result, Err(JjError::Cancelled)),
            "Cancelled autentico solo dopo cleanup confermato"
        );
        assert_eq!(workers82()?, 0);
        assert_ne!(
            identity82(origin.0)?,
            Some(origin.1),
            "istanza del child osservata non più presente, non kill su PID riusato"
        );
        // File/lease ancora posseduti mentre tutte le assertion precedenti
        // hanno luogo: il normale rilascio lock non spiega il ritorno.
        let other = File::open(&path)?;
        assert_eq!(
            other.metadata()?.ino(),
            lock.as_ref().unwrap().metadata()?.ino()
        );
        let error = flock82(&other, libc::LOCK_EX | libc::LOCK_NB).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        flock82(lock.as_ref().unwrap(), libc::LOCK_UN)?;
        drop(lock.take());
        let fresh = cli.run(&root, &["status"], &AtomicBool::new(false))?;
        assert!(fresh.success());
        assert!(fresh.message().contains("città.txt"));
        assert_eq!(workers82()?, 0);
        assert_eq!(std::fs::read_to_string(text)?, "prima 😀\n");
        eprintln!(
            "JJ_CANCEL_REALE82:blocked_native_flock=true;readers_before=2;cancel_collected_while_lock_held=true;readers_after=0;fresh_status=true"
        );
        Ok(())
    }));
    // Tentativo indipendente sul job esistente anche se scenario panic/Err.
    cancel.store(true, Ordering::Release);
    let ready_held = ready82(&job, Duration::from_secs(10));
    drop(lock.take()); // rilascio FD owned anche se flock fallisce, non rollback.
    let ready = ready_held || ready82(&job, Duration::from_secs(10));
    if !ready {
        if let Some(handle) = job.take() {
            retain82(handle);
        }
        let residue = fixture.keep();
        eprintln!(
            "JJ_CANCEL_REALE82:fixture_preservata={}; job incerto conservato",
            residue.display()
        );
        return Err(io::Error::other("job nativo82 non raccolto, fixture conservata").into());
    }
    // Consumare il vero handle prima di propagare un errore. Collector non
    // inizia un altro comando, non fabbrica Cancelled e non nasconde panic.
    let collected = job.take().map(|h| h.join());
    if !matches!(
        collected,
        None | Some(Ok(Ok(_))) | Some(Ok(Err(JjError::Cancelled)))
    ) {
        let _ = fixture.keep();
        return Err(io::Error::other("fallback job JJ82 fallito; esito da verificare").into());
    }
    match scenario {
        Ok(Ok(())) => fixture.close()?,
        failure => {
            let residue = fixture.keep();
            eprintln!(
                "JJ_CANCEL_REALE82:fixture_preservata={}; scenario fallito",
                residue.display()
            );
            return match failure {
                Ok(Err(error)) => Err(error),
                Err(_) => Err(io::Error::other("scenario JJ82 panic").into()),
                _ => unreachable!(),
            };
        }
    }
    eprintln!(
        "JJ_CANCEL_REALE82:sequenza_completa=native_jj_owned_flock_wait_two_readers_cancel_collect_held_lock_child_identity_absent_fresh_status"
    );
    Ok(())
}
