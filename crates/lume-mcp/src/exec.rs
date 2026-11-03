//! Esecuzione controllata di un singolo programma (solo Windows).
//!
//! Non viene invocata una shell in modo implicito: `program` e `args` restano
//! separati. Il figlio entra in un Job Object che termina l'albero quando il
//! future viene cancellato, scade o finisce.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
pub const MAX_TIMEOUT_SECS: u64 = 600;
pub const TAIL_BYTES: usize = 64 * 1024;
#[cfg(windows)]
const DRAIN_GRACE: Duration = Duration::from_millis(1500);

pub fn timeout_from(secs: Option<u64>) -> Result<Duration, String> {
    match secs {
        None => Ok(Duration::from_secs(DEFAULT_TIMEOUT_SECS)),
        Some(secs) if (1..=MAX_TIMEOUT_SECS).contains(&secs) => Ok(Duration::from_secs(secs)),
        Some(_) => Err(format!("timeout_secs va da 1 a {MAX_TIMEOUT_SECS}")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub elapsed_ms: u64,
}

impl RunReport {
    pub fn to_json(&self) -> Value {
        json!({
            "exit_code": self.exit_code,
            "timed_out": self.timed_out,
            "stdout": self.stdout,
            "stderr": self.stderr,
            "stdout_truncated": self.stdout_truncated,
            "stderr_truncated": self.stderr_truncated,
            "elapsed_ms": self.elapsed_ms,
        })
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, Default)]
struct Tail {
    bytes: Vec<u8>,
    truncated: bool,
}

#[cfg(any(windows, test))]
impl Tail {
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() > TAIL_BYTES {
            let excess = self.bytes.len() - TAIL_BYTES;
            self.bytes.drain(..excess);
            self.truncated = true;
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// Esegue il comando. Il future è cancellabile e la sua cancellazione chiude
/// il Job Object che possiede il processo e i suoi discendenti.
pub async fn run(spec: RunSpec) -> Result<RunReport, String> {
    imp::run(spec).await
}

#[cfg(not(windows))]
mod imp {
    use super::{RunReport, RunSpec};

    pub(super) async fn run(spec: RunSpec) -> Result<RunReport, String> {
        drop(spec);
        Err("run_command è disponibile solo su Windows".to_owned())
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use std::process::Stdio;
    use std::time::Instant;

    use tokio::io::{AsyncRead, AsyncReadExt};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    use super::{DRAIN_GRACE, RunReport, RunSpec, Tail};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    struct JobGuard(HANDLE);

    // SAFETY: è un handle kernel indipendente dalla memoria del processo.
    unsafe impl Send for JobGuard {}

    impl JobGuard {
        fn new() -> Result<Self, String> {
            // SAFETY: nome e attributi null sono validi; `info` vive per la chiamata.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(format!(
                        "Job Object non creato: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let configured = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    std::ptr::addr_of!(info).cast(),
                    u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                        .unwrap_or(u32::MAX),
                );
                if configured == 0 {
                    let error = std::io::Error::last_os_error();
                    CloseHandle(handle);
                    return Err(format!("Job Object non configurato: {error}"));
                }
                Ok(Self(handle))
            }
        }

        fn assign(&self, child: &tokio::process::Child) -> Result<(), String> {
            let process = child.raw_handle().ok_or_else(|| {
                "il processo è terminato prima dell'assegnazione al Job Object".to_owned()
            })?;
            // SAFETY: l'handle appartiene al processo figlio ancora vivo.
            let assigned = unsafe { AssignProcessToJobObject(self.0, process as HANDLE) };
            if assigned == 0 {
                return Err(format!(
                    "il processo non entra nel Job Object: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(())
        }
    }

    impl Drop for JobGuard {
        fn drop(&mut self) {
            // SAFETY: l'handle è valido e viene chiuso una sola volta.
            unsafe { CloseHandle(self.0) };
        }
    }

    async fn read_into<R: AsyncRead + Unpin>(reader: Option<R>, tail: &mut Tail) {
        let Some(mut reader) = reader else { return };
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    if let Some(chunk) = buffer.get(..count) {
                        tail.push(chunk);
                    }
                }
            }
        }
    }

    pub(super) async fn run(spec: RunSpec) -> Result<RunReport, String> {
        let started = Instant::now();
        let job = JobGuard::new()?;
        let mut command = tokio::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| format!("«{}» non parte: {error}", spec.program))?;
        job.assign(&child)?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let mut out = Tail::default();
        let mut err = Tail::default();
        let mut status = None;
        {
            let timeout = spec.timeout;
            let waiter = async {
                let result = tokio::time::timeout(timeout, child.wait()).await;
                drop(job);
                result
            };
            let all = async {
                let (_, _, status) = tokio::join!(
                    read_into(stdout, &mut out),
                    read_into(stderr, &mut err),
                    waiter
                );
                status
            };
            if let Ok(done) = tokio::time::timeout(spec.timeout + DRAIN_GRACE, all).await {
                status = Some(done);
            }
        }
        let (exit_code, timed_out) = match status {
            Some(Ok(Ok(exit))) => (exit.code(), false),
            Some(Ok(Err(error))) => return Err(format!("attesa del processo fallita: {error}")),
            Some(Err(_)) | None => (None, true),
        };
        Ok(RunReport {
            exit_code,
            timed_out,
            stdout: out.text(),
            stderr: err.text(),
            stdout_truncated: out.truncated,
            stderr_truncated: err.truncated,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    #[cfg(windows)]
    use std::{fs, path::Path, time::Instant};

    #[test]
    fn timeout_ha_un_predefinito_e_un_tetto() {
        assert_eq!(timeout_from(None).unwrap(), Duration::from_secs(60));
        assert_eq!(timeout_from(Some(5)).unwrap(), Duration::from_secs(5));
        assert_eq!(timeout_from(Some(600)).unwrap(), Duration::from_secs(600));
        assert!(timeout_from(Some(0)).is_err());
        assert!(timeout_from(Some(601)).is_err());
    }

    #[test]
    fn tail_conserva_ultimi_byte_e_segnala_il_taglio() {
        let mut tail = Tail::default();
        tail.push(&vec![b'a'; TAIL_BYTES]);
        assert!(!tail.truncated);
        tail.push(b"fine");
        assert!(tail.truncated);
        assert_eq!(tail.bytes.len(), TAIL_BYTES);
        assert!(tail.text().ends_with("fine"));
    }

    #[test]
    fn report_diventa_json_con_i_campi_dichiarati() {
        let report = RunReport {
            exit_code: Some(0),
            timed_out: false,
            stdout: "ok".to_owned(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            elapsed_ms: 12,
        };
        let value = report.to_json();
        assert_eq!(value["exit_code"], 0);
        assert_eq!(value["stdout"], "ok");
        assert_eq!(value["elapsed_ms"], 12);
        assert_eq!(
            RunReport {
                exit_code: None,
                timed_out: true,
                ..report
            }
            .to_json()["exit_code"],
            Value::Null
        );
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn run_e_disabilitato_fuori_windows() {
        let result = run(RunSpec {
            program: "echo".to_owned(),
            args: vec!["no".to_owned()],
            cwd: PathBuf::from("."),
            timeout: Duration::from_secs(1),
        })
        .await;
        assert!(result.unwrap_err().contains("solo su Windows"));
    }

    #[cfg(windows)]
    fn spec(cwd: &Path, args: &[&str], seconds: u64) -> RunSpec {
        RunSpec {
            program: "cmd".to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: cwd.to_path_buf(),
            timeout: Duration::from_secs(seconds),
        }
    }

    #[cfg(windows)]
    fn scrivi_script_nipote(folder: &Path) {
        fs::write(
            folder.join("nipote.cmd"),
            "@echo off\r\nstart /B \"\" cmd /C \"ping -n 4 127.0.0.1 >nul & echo vivo>marker.txt\"\r\nping -n 60 127.0.0.1 >nul\r\n",
        )
        .unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn comando_terminato_restituisce_codice_e_output() {
        let dir = tempfile::tempdir().unwrap();
        let report = run(spec(dir.path(), &["/C", "echo ciao"], 20))
            .await
            .unwrap();
        assert_eq!(report.exit_code, Some(0));
        assert!(!report.timed_out);
        assert!(report.stdout.contains("ciao"));
        assert!(report.stderr.is_empty());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn stderr_e_codice_di_uscita_restano_separati() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("errore.cmd"),
            "@echo off\r\necho sbagliato 1>&2\r\nexit /b 3\r\n",
        )
        .unwrap();
        let report = run(spec(dir.path(), &["/C", "errore.cmd"], 20))
            .await
            .unwrap();
        assert_eq!(report.exit_code, Some(3));
        assert!(report.stderr.contains("sbagliato"));
        assert!(!report.stdout.contains("sbagliato"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cwd_e_il_percorso_richiesto() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("lavoro");
        fs::create_dir(&work).unwrap();
        let report = run(spec(&work, &["/C", "cd"], 20)).await.unwrap();
        assert!(
            report.stdout.trim_end().ends_with("lavoro"),
            "{}",
            report.stdout
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn programma_inesistente_riporta_il_nome() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = spec(dir.path(), &[], 5);
        request.program = "programma-che-non-esiste-lume".to_owned();
        assert!(
            run(request)
                .await
                .unwrap_err()
                .contains("programma-che-non-esiste-lume")
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn output_oltre_il_tetto_conserva_la_coda_e_lo_segnala() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("tanto.cmd"),
            "@echo off\r\nfor /L %%i in (1,1,20000) do @echo riga-%%i\r\n",
        )
        .unwrap();
        let report = run(spec(dir.path(), &["/C", "tanto.cmd"], 60))
            .await
            .unwrap();
        assert!(report.stdout_truncated);
        assert!(report.stdout.len() <= TAIL_BYTES + 8);
        assert!(report.stdout.contains("riga-20000"));
        assert!(!report.stdout.contains("riga-1\r\n"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn timeout_uccide_l_albero_e_non_sopravvive_il_nipote() {
        let dir = tempfile::tempdir().unwrap();
        scrivi_script_nipote(dir.path());
        let start = Instant::now();
        let report = run(spec(dir.path(), &["/C", "nipote.cmd"], 1))
            .await
            .unwrap();
        assert!(report.timed_out);
        assert_eq!(report.exit_code, None);
        assert!(start.elapsed() < Duration::from_secs(8));
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert!(!dir.path().join("marker.txt").exists());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cancellare_il_future_uccide_l_albero() {
        let dir = tempfile::tempdir().unwrap();
        scrivi_script_nipote(dir.path());
        let mut jobs: tokio::task::JoinSet<Result<RunReport, String>> = tokio::task::JoinSet::new();
        let handle = jobs.spawn(run(spec(dir.path(), &["/C", "nipote.cmd"], 120)));
        tokio::time::sleep(Duration::from_millis(600)).await;
        handle.abort();
        while jobs.join_next().await.is_some() {}
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert!(!dir.path().join("marker.txt").exists());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_fine_comando_si_ferma_il_nipote_lasciato_in_giro() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("lascia.cmd"),
            "@echo off\r\nstart /B \"\" cmd /C \"ping -n 4 127.0.0.1 >nul & echo vivo>marker.txt\"\r\necho fatto\r\n",
        )
        .unwrap();
        let report = run(spec(dir.path(), &["/C", "lascia.cmd"], 30))
            .await
            .unwrap();
        assert_eq!(report.exit_code, Some(0));
        assert!(report.stdout.contains("fatto"));
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert!(!dir.path().join("marker.txt").exists());
    }
}
