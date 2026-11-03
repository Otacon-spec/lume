//! Eccezione CLI dichiarata: push, cambio branch, stash apply/pop, merge
//! conflittuale. Discovery senza sonde, comandi solo su richiesta.
//! Lettori posseduti e joinati su success/error/cancel; nessun lavoro idle.
//!
//! Linux: un supervisor isolato è subreaper e raccoglie anche discendenti che
//! cambiano sessione; il gruppo Git viene terminato prima di raccogliere il
//! leader (WNOWAIT evita un PID riutilizzato). macOS usa il gruppo Unix ma non
//! può adottare processi escaped. Windows crea Git sospeso, lo assegna a un
//! Job Object kill-on-close e solo dopo riprende il thread principale. La
//! prova nativa Windows resta necessaria.

use crate::GitError;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(25);
const MAX_CAPTURE: usize = 64 * 1024;
const DRAIN_WAIT: Duration = Duration::from_millis(500);
const READ_WAIT: Duration = Duration::from_millis(5);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_SUSPENDED: u32 = 0x0000_0004;

#[derive(Clone, Debug)]
pub struct GitCli {
    program: PathBuf,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliOutput {
    pub stdout: String,
    pub stderr: String,
    /// None se terminato da segnale.
    pub code: Option<i32>,
}
impl CliOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
    pub fn failure_text(&self) -> String {
        let text = [self.stderr.trim(), self.stdout.trim()]
            .into_iter()
            .find(|t| !t.is_empty());
        match (text, self.code) {
            (Some(t), _) => t.to_owned(),
            (None, Some(code)) => format!("git è uscito con codice {code}"),
            (None, None) => "git è stato terminato da un segnale".into(),
        }
    }
}
impl GitCli {
    /// Stat sulle sole voci assolute, senza --version o altri processi.
    pub fn locate() -> Option<Self> {
        let path = std::env::var_os("PATH")?;
        Self::locate_in(&std::env::split_paths(&path).collect::<Vec<_>>())
    }
    pub fn locate_in(dirs: &[PathBuf]) -> Option<Self> {
        let name = if cfg!(windows) { "git.exe" } else { "git" };
        dirs.iter()
            .filter(|p| !p.as_os_str().is_empty() && p.is_absolute())
            .map(|p| p.join(name))
            .find(|p| p.is_file())
            .map(|program| Self { program })
    }
    pub fn program(&self) -> &Path {
        &self.program
    }
    /// Exit nonzero è CliOutput; Cancelled resta distinto da guasto.
    pub fn run(
        &self,
        dir: &Path,
        args: &[&str],
        cancel: &AtomicBool,
    ) -> Result<CliOutput, GitError> {
        self.run_with_config(dir, args, cancel, None)
    }
    /// Override solo nel figlio, senza scrivere config o cambiare env globale.
    /// KEY/VALUE distinti funzionano anche con '=' nel nome del remote.
    pub(crate) fn run_with_config(
        &self,
        dir: &Path,
        args: &[&str],
        cancel: &AtomicBool,
        config: Option<(&str, &str)>,
    ) -> Result<CliOutput, GitError> {
        if cancel.load(Ordering::Acquire) {
            return Err(GitError::Cancelled);
        }
        #[cfg(target_os = "linux")]
        let mut command = {
            let helper = git_supervisor_path().map_err(cli_io)?;
            let mut command = Command::new(helper);
            command
                .env("LUME_INTERNAL_GIT_MODE", "1")
                .env("LUME_INTERNAL_GIT_PROGRAM", &self.program)
                .env("LUME_INTERNAL_GIT_ARG_COUNT", args.len().to_string());
            for (index, argument) in args.iter().enumerate() {
                command.env(format!("LUME_INTERNAL_GIT_ARG_{index}"), argument);
            }
            command
        };
        #[cfg(not(target_os = "linux"))]
        let mut command = {
            let mut command = Command::new(&self.program);
            command.args(args);
            command
        };
        command
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .env("GIT_MERGE_AUTOEDIT", "no");
        if let Some((key, value)) = config {
            let count = match std::env::var("GIT_CONFIG_COUNT") {
                Ok(raw) => raw
                    .parse::<usize>()
                    .map_err(|_| GitError::Cli("Conteggio configurazione Git non valido".into()))?,
                Err(std::env::VarError::NotPresent) => 0,
                Err(_) => {
                    return Err(GitError::Cli(
                        "Conteggio configurazione Git non valido".into(),
                    ));
                }
            };
            let next = count
                .checked_add(1)
                .ok_or_else(|| GitError::Cli("Troppi elementi di configurazione Git".into()))?;
            command
                .env("GIT_CONFIG_COUNT", next.to_string())
                .env(format!("GIT_CONFIG_KEY_{count}"), key)
                .env(format!("GIT_CONFIG_VALUE_{count}"), value);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        }
        let child = command
            .spawn()
            .map_err(|e| GitError::Cli(format!("{}: {e}", self.program.display())))?;
        let mut owner = ProcessOwner::new(child, cfg!(target_os = "linux")).map_err(cli_io)?;
        let stdout = owner
            .child
            .stdout
            .take()
            .map(drain)
            .transpose()
            .map_err(cli_io)?;
        let stderr = owner
            .child
            .stderr
            .take()
            .map(drain)
            .transpose()
            .map_err(cli_io)?;
        let code = loop {
            // Annullamento ha precedenza anche se il comando esce nello
            // stesso istante; Drop termina gruppo/leader e raccoglie Child.
            if cancel.load(Ordering::Acquire) {
                return Err(GitError::Cancelled);
            }
            if owner.finished().map_err(cli_io)? {
                if !owner.supervised {
                    owner.stop_group().map_err(cli_io)?;
                }
                break owner.wait().map_err(cli_io)?.code();
            }
            std::thread::sleep(POLL);
        };
        Ok(CliOutput {
            stdout: stdout
                .map(|c| c.finish(DRAIN_WAIT))
                .transpose()
                .map_err(cli_io)?
                .unwrap_or_default(),
            stderr: stderr
                .map(|c| c.finish(DRAIN_WAIT))
                .transpose()
                .map_err(cli_io)?
                .unwrap_or_default(),
            code,
        })
    }
}

#[cfg(target_os = "linux")]
fn git_supervisor_path() -> io::Result<PathBuf> {
    let current = std::env::current_exe()?;
    let parent = current
        .parent()
        .ok_or_else(|| io::Error::other("current executable has no parent directory"))?;
    let mut candidates = vec![parent.join("lume-git-supervisor")];
    if parent.file_name().is_some_and(|name| name == "deps")
        && let Some(target_dir) = parent.parent()
    {
        candidates.push(target_dir.join("lume-git-supervisor"));
    }
    candidates
        .into_iter()
        .find_map(|candidate| {
            std::fs::metadata(&candidate)
                .ok()
                .filter(|metadata| metadata.is_file())
                .and_then(|_| std::fs::canonicalize(candidate).ok())
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "lume-git-supervisor non è installato accanto all'applicazione",
            )
        })
}
fn cli_io(error: io::Error) -> GitError {
    GitError::Cli(error.to_string())
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_job {
    use std::{
        io,
        mem::size_of,
        ptr, thread,
        time::{Duration, Instant},
    };

    use windows_sys::Win32::{
        Foundation::{CloseHandle, FALSE, HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
                QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
            },
            Threading::{
                OpenProcess, OpenThread, PROCESS_SET_QUOTA, PROCESS_TERMINATE, ResumeThread,
                THREAD_SUSPEND_RESUME,
            },
        },
    };

    const TERMINATE_WAIT: Duration = Duration::from_secs(5);
    const POLL: Duration = Duration::from_millis(5);

    pub(super) struct WindowsJob(Handle);

    struct Handle(HANDLE);

    impl Handle {
        fn new(raw: HANDLE) -> io::Result<Self> {
            if raw.is_null() {
                Err(io::Error::last_os_error())
            } else {
                Ok(Self(raw))
            }
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: this wrapper uniquely owns a live Win32 handle.
            unsafe { CloseHandle(self.0) };
        }
    }

    impl WindowsJob {
        pub(super) fn assign(process_id: u32) -> io::Result<Self> {
            // SAFETY: null security attributes and name request a private
            // unnamed Job Object; no borrowed pointer is retained.
            let job = Handle::new(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) })?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: limits is initialized and its exact ABI size is passed.
            let configured = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                        .map_err(|_| io::Error::other("Job Object struct size overflow"))?,
                )
            };
            if configured == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: the process was created by this process and remains
            // suspended; these are the documented assignment access rights.
            let process = Handle::new(unsafe {
                OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, FALSE, process_id)
            })?;
            // SAFETY: both handles are live and uniquely owned above.
            if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(job))
        }

        pub(super) fn terminate_and_wait(&self) -> io::Result<()> {
            if self.active_processes()? == 0 {
                return Ok(());
            }
            // SAFETY: self owns a valid Job Object; termination applies only
            // to processes assigned to this Git command.
            if unsafe { TerminateJobObject(self.0.0, 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let deadline = Instant::now() + TERMINATE_WAIT;
            while self.active_processes()? != 0 {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Git Job Object termination timed out",
                    ));
                }
                thread::sleep(POLL);
            }
            Ok(())
        }

        fn active_processes(&self) -> io::Result<u32> {
            let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            let mut returned = 0u32;
            // SAFETY: info is a valid, writable output buffer of the size
            // provided, and self owns the Job Object handle.
            if unsafe {
                QueryInformationJobObject(
                    self.0.0,
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    u32::try_from(size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>())
                        .map_err(|_| io::Error::other("Job accounting size overflow"))?,
                    &mut returned,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(info.ActiveProcesses)
        }
    }

    pub(super) fn resume_suspended_child(process_id: u32) -> io::Result<()> {
        // SAFETY: snapshot flags are valid; process ID is unused for thread
        // snapshots and zero requests all system threads.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = Handle(snapshot);
        let mut entry = THREADENTRY32 {
            dwSize: u32::try_from(size_of::<THREADENTRY32>())
                .map_err(|_| io::Error::other("thread entry size overflow"))?,
            ..THREADENTRY32::default()
        };
        // SAFETY: snapshot is live and entry is a writable ABI struct with
        // dwSize initialized as required by Toolhelp.
        let mut found = unsafe { Thread32First(snapshot.0, &mut entry) };
        while found != FALSE {
            if entry.th32OwnerProcessID == process_id {
                // SAFETY: this thread belongs to our newly created suspended
                // child and requires only the suspend/resume access right.
                let thread = Handle::new(unsafe {
                    OpenThread(THREAD_SUSPEND_RESUME, FALSE, entry.th32ThreadID)
                })?;
                // SAFETY: thread is a live handle opened with suspend/resume.
                let previous_count = unsafe { ResumeThread(thread.0) };
                if previous_count == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                if previous_count != 1 {
                    return Err(io::Error::other(
                        "Git primary thread suspension count changed unexpectedly",
                    ));
                }
                return Ok(());
            }
            // SAFETY: snapshot and entry remain valid for the next Toolhelp row.
            found = unsafe { Thread32Next(snapshot.0, &mut entry) };
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "could not find the suspended Git primary thread",
        ))
    }
}

#[cfg(all(test, windows))]
mod windows_job_tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::zombie_processes)]

    use std::{process::Command, sync::atomic::AtomicBool, time::Duration};

    use windows_sys::Win32::{
        Foundation::{CloseHandle, FALSE, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };

    use super::GitCli;

    #[test]
    fn job_object_termina_il_discendente_dopo_l_uscita_del_leader() {
        let root = tempfile::tempdir().unwrap();
        let executable = std::env::current_exe().unwrap();
        let cli = GitCli {
            program: executable,
        };
        let start = std::time::Instant::now();
        let output = cli
            .run(
                root.path(),
                &[
                    "--ignored",
                    "--exact",
                    "cli::windows_job_tests::spawn_fixture",
                    "--nocapture",
                ],
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(output.success(), "{}", output.failure_text());
        assert!(start.elapsed() < Duration::from_secs(15));

        let pid = std::fs::read_to_string(root.path().join("descendant.pid"))
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        // SAFETY: pid was written by the descendant fixture; only the
        // SYNCHRONIZE right is requested to verify its final process state.
        let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, FALSE, pid) };
        if !process.is_null() {
            // SAFETY: process is a live handle opened above; zero timeout is
            // a nonblocking status check.
            let result = unsafe { WaitForSingleObject(process, 0) };
            // SAFETY: process is the unique handle returned by OpenProcess.
            unsafe { CloseHandle(process) };
            assert_eq!(
                result, WAIT_OBJECT_0,
                "Job Object left descendant PID {pid} running"
            );
        }
    }

    #[test]
    #[ignore = "fixture invocata da job_object_termina_il_discendente_dopo_l_uscita_del_leader"]
    fn spawn_fixture() {
        let descendant = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "cli::windows_job_tests::sleep_fixture",
                "--nocapture",
            ])
            .spawn()
            .unwrap();
        std::fs::write(
            std::env::current_dir().unwrap().join("descendant.pid"),
            descendant.id().to_string(),
        )
        .unwrap();
    }

    #[test]
    #[ignore = "fixture figlia terminata dal Job Object del test"]
    fn sleep_fixture() {
        std::thread::sleep(Duration::from_secs(30));
    }
}

#[cfg(windows)]
use windows_job::WindowsJob;

/// Termina prima di wait/reap. Leader ancora riservato mentre si segnala
/// il gruppo. Drop copre errori di setup, poll, cattura e annullamento.
struct ProcessOwner {
    child: Child,
    supervised: bool,
    reaped: bool,
    #[cfg(unix)]
    group: Option<libc::pid_t>,
    #[cfg(windows)]
    job: Option<WindowsJob>,
}
impl ProcessOwner {
    fn new(child: Child, supervised: bool) -> io::Result<Self> {
        #[cfg(windows)]
        let mut child = child;
        #[cfg(windows)]
        let job = {
            let job = match WindowsJob::assign(child.id()) {
                Ok(job) => job,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            };
            if let Err(error) = windows_job::resume_suspended_child(child.id()) {
                let _ = job.terminate_and_wait();
                let _ = child.wait();
                return Err(error);
            }
            Some(job)
        };
        Ok(Self {
            #[cfg(unix)]
            group: (!supervised)
                .then(|| libc::pid_t::try_from(child.id()).ok())
                .flatten(),
            child,
            supervised,
            reaped: false,
            #[cfg(windows)]
            job,
        })
    }
    fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }
    fn finished(&mut self) -> io::Result<bool> {
        #[cfg(unix)]
        {
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
            // SAFETY: buffer valido, PID del nostro Child, WNOWAIT non
            // raccoglie il leader e WNOHANG non blocca il worker.
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id(),
                    info.as_mut_ptr(),
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: waitid ha inizializzato siginfo (zero se non finito).
            Ok(unsafe { info.assume_init().si_pid() } != 0)
        }
        #[cfg(windows)]
        {
            self.child.try_wait().map(|s| s.is_some())
        }
    }
    fn stop_group(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        if let Some(group) = self.group.take() {
            // SAFETY: gruppo creato dal nostro Command, leader non reaped;
            // non usare mai il gruppo del processo Lume.
            if unsafe { libc::kill(-group, libc::SIGKILL) } != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
        }
        #[cfg(windows)]
        if let Some(job) = self.job.as_ref() {
            job.terminate_and_wait()?;
        }
        Ok(())
    }
}
impl Drop for ProcessOwner {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        if self.supervised {
            #[cfg(target_os = "linux")]
            {
                // The isolated supervisor catches SIGTERM, kills Git's
                // separate group, and reaps adopted descendants before exit.
                // SAFETY: child.id() is the helper process spawned by us.
                let _ = unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
                let _ = self.child.wait();
            }
            return;
        }
        let _ = self.stop_group();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Ogni lettore ha stop e join, anche se un helper conserva la pipe aperta.
/// Source non bloccante: il flag è osservato fra letture, non su thread staccati.
struct Capture {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<io::Result<Vec<u8>>>>,
}
impl Capture {
    fn finish(mut self, wait: Duration) -> io::Result<String> {
        let start = Instant::now();
        while self.handle.as_ref().is_some_and(|h| !h.is_finished()) && start.elapsed() < wait {
            std::thread::sleep(READ_WAIT);
        }
        self.stop.store(true, Ordering::Release);
        let Some(handle) = self.handle.take() else {
            return Ok(String::new());
        };
        let bytes = handle.join().map_err(|_| {
            io::Error::other("Il lettore della pipe Git è terminato con un panic")
        })??;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
trait Pipe: Read + Send + 'static {
    fn prepare(&self) -> io::Result<()>;
    fn available(&mut self, bytes: &mut [u8]) -> io::Result<usize>;
}
#[cfg(unix)]
impl<R: Read + Send + 'static + std::os::fd::AsRawFd> Pipe for R {
    fn prepare(&self) -> io::Result<()> {
        // SAFETY: fd posseduto dalla source, mantenuto vivo nel lettore.
        let flags = unsafe { libc::fcntl(self.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: il fd è posseduto dalla source e resta aperto; i flag esistenti sono letti e preservati.
        if unsafe { libc::fcntl(self.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    fn available(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.read(bytes)
    }
}
#[cfg(windows)]
impl<R: Read + Send + 'static + std::os::windows::io::AsRawHandle> Pipe for R {
    fn prepare(&self) -> io::Result<()> {
        Ok(())
    }
    fn available(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        use windows_sys::Win32::System::Pipes::PeekNamedPipe;
        let mut ready = 0u32;
        // SAFETY: handle della pipe posseduta dal solo lettore; peek non
        // consuma. Si legge solo il numero già disponibile, senza bloccare.
        if unsafe {
            PeekNamedPipe(
                self.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut ready,
                std::ptr::null_mut(),
            )
        } == 0
        {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::BrokenPipe {
                return Ok(0);
            }
            return Err(e);
        }
        if ready == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let count = bytes.len().min(ready as usize);
        self.read(
            bytes
                .get_mut(..count)
                .ok_or_else(|| io::Error::other("Intervallo pipe non valido"))?,
        )
    }
}
fn drain<R: Pipe>(mut source: R) -> io::Result<Capture> {
    source.prepare()?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let handle = std::thread::Builder::new()
        .name("lume-git-pipe".into())
        .spawn(move || {
            let mut kept = Vec::new();
            let mut chunk = [0u8; 4096];
            while !flag.load(Ordering::Acquire) {
                match source.available(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        let take = read.min(MAX_CAPTURE.saturating_sub(kept.len()));
                        if let Some(part) = chunk.get(..take) {
                            kept.extend_from_slice(part);
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(READ_WAIT)
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(kept)
        })?;
    Ok(Capture {
        stop,
        handle: Some(handle),
    })
}
