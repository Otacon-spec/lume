#[cfg(not(windows))]
use std::process::Stdio;
use std::{
    collections::VecDeque,
    ffi::OsString,
    io,
    path::PathBuf,
    process::ExitStatus,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        mpsc::{self, RecvTimeoutError, Sender},
    },
    time::Duration,
};

use serde_json::Value;
#[cfg(not(windows))]
use tokio::process::Child as PlatformChild;
use tokio::{io::AsyncReadExt, task::JoinHandle};

use crate::{ClientError, ClientTransport, TransportConfig, TransportEvents, WorkspaceTrust};

pub const STDERR_CAPACITY: usize = 256 * 1024;
const STDERR_READ_BUFFER: usize = 8 * 1024;
const REAPER_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(windows)]
mod platform {
    use std::{
        cmp::Ordering,
        env,
        ffi::{OsStr, OsString, c_void},
        fs::File as StdFile,
        io,
        mem::size_of,
        os::windows::{
            ffi::{OsStrExt, OsStringExt},
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
            process::ExitStatusExt,
        },
        path::{Path, PathBuf},
        process::ExitStatus,
        ptr::{self, null, null_mut},
    };

    use windows_sys::Win32::{
        Foundation::{
            DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, HANDLE_FLAG_INHERIT,
            SetHandleInformation, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Globalization::{CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN, CompareStringOrdinal},
        Security::SECURITY_ATTRIBUTES,
        System::{
            JobObjects::{
                CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject,
            },
            Pipes::CreatePipe,
            SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW},
            Threading::{
                CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
                EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess, INFINITE,
                InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
                STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
            },
        },
    };

    use super::ServerCommand;

    pub(super) struct ProcessOwner {
        job: OwnedHandle,
    }

    pub(super) struct Child {
        process: OwnedHandle,
        process_id: u32,
        status: Option<ExitStatus>,
    }

    pub(super) struct SpawnedProcess {
        pub(super) child: Child,
        pub(super) owner: ProcessOwner,
        pub(super) stdin: tokio::fs::File,
        pub(super) stdout: tokio::fs::File,
        pub(super) stderr: tokio::fs::File,
    }

    struct AttributeList {
        storage: Vec<usize>,
        initialized: bool,
    }

    struct EffectiveEnvironment {
        entries: Vec<(Vec<u16>, Vec<u16>)>,
        path_changed: bool,
    }

    impl AttributeList {
        fn new(attribute_count: u32) -> io::Result<Self> {
            let mut bytes = 0;
            // SAFETY: con lista nulla la funzione scrive solo la dimensione richiesta in `bytes`, una locale viva, e non tocca altra memoria.
            unsafe {
                InitializeProcThreadAttributeList(null_mut(), attribute_count, 0, &mut bytes);
            }
            if bytes == 0 {
                return Err(io::Error::last_os_error());
            }
            let words = bytes.div_ceil(size_of::<usize>());
            let mut list = Self {
                storage: vec![0; words],
                initialized: false,
            };
            // SAFETY: `list.storage` ha `words` parole da `usize`, cioè almeno i `bytes` richiesti dal passo precedente, ed è allineata come un `usize`; `bytes` è una locale viva.
            let initialized = unsafe {
                InitializeProcThreadAttributeList(list.as_mut_ptr(), attribute_count, 0, &mut bytes)
            };
            if initialized == 0 {
                return Err(io::Error::last_os_error());
            }
            list.initialized = true;
            Ok(list)
        }

        fn as_mut_ptr(&mut self) -> *mut c_void {
            self.storage.as_mut_ptr().cast()
        }

        fn update(&mut self, attribute: usize, handles: &[HANDLE]) -> io::Result<()> {
            // SAFETY: `self` è una lista inizializzata da `new` e `handles` è una slice viva per tutta la chiamata; la lista ne conserva solo il puntatore, quindi `spawn` tiene vivi gli array fino a `CreateProcessW`.
            let updated = unsafe {
                UpdateProcThreadAttribute(
                    self.as_mut_ptr(),
                    0,
                    attribute,
                    handles.as_ptr().cast(),
                    size_of_val(handles),
                    null_mut(),
                    null(),
                )
            };
            if updated == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for AttributeList {
        fn drop(&mut self) {
            if self.initialized {
                // SAFETY: si arriva qui solo con `initialized` vero: la lista è stata inizializzata da `new` e non è mai stata cancellata prima.
                unsafe { DeleteProcThreadAttributeList(self.as_mut_ptr()) };
            }
        }
    }

    impl Child {
        pub(super) fn id(&self) -> Option<u32> {
            self.status.is_none().then_some(self.process_id)
        }

        pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            if let Some(status) = self.status {
                return Ok(Some(status));
            }
            // SAFETY: `self.process` è un handle di processo valido posseduto da `self`; il timeout 0 non blocca.
            match unsafe { WaitForSingleObject(self.process.as_raw_handle() as HANDLE, 0) } {
                WAIT_TIMEOUT => Ok(None),
                WAIT_OBJECT_0 => {
                    let mut exit_code = 0;
                    // SAFETY: `self.process` è un handle valido e `exit_code` è una locale viva e scrivibile.
                    let read = unsafe {
                        GetExitCodeProcess(self.process.as_raw_handle() as HANDLE, &mut exit_code)
                    };
                    if read == 0 {
                        return Err(io::Error::last_os_error());
                    }
                    let status = ExitStatus::from_raw(exit_code);
                    self.status = Some(status);
                    Ok(Some(status))
                }
                WAIT_FAILED => Err(io::Error::last_os_error()),
                result => Err(io::Error::other(format!(
                    "unexpected process wait result {result}"
                ))),
            }
        }

        pub(super) fn start_kill(&mut self) -> io::Result<()> {
            if self.try_wait()?.is_some() {
                return Ok(());
            }
            // SAFETY: `self.process` è un handle di processo valido posseduto da `self`.
            let terminated = unsafe { TerminateProcess(self.process.as_raw_handle() as HANDLE, 1) };
            if terminated == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        /// Attende la fine del processo **senza poll**.
        ///
        /// Prima faceva `try_wait` in ciclo con `sleep(10ms)`: cento risvegli
        /// al secondo per ogni server vivo, per tutta la sua vita. È la stessa
        /// classe di costo tolta dal terminale con la subscription push, e
        /// contraddice «nessun lavoro da inattivo» appena un language server è
        /// attivo — cioè in uso normale.
        ///
        /// Ora è il sistema operativo a svegliarci: `WaitForSingleObject` con
        /// attesa infinita su un thread di `spawn_blocking`, lo stesso idioma
        /// già usato per la `recv()` bloccante del PTY e per gix.
        ///
        /// **L'handle viene duplicato** perché il thread bloccante può
        /// sopravvivere al future: se questo viene droppato, `self` sparisce e
        /// con lui l'handle originale, che Windows potrebbe riassegnare a
        /// tutt'altro oggetto. La copia è di proprietà del thread e si chiude
        /// con lui.
        pub(super) async fn wait(&mut self) -> io::Result<ExitStatus> {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            let duplicato = duplicate_handle(self.process.as_raw_handle() as HANDLE)?;
            tokio::task::spawn_blocking(move || -> io::Result<()> {
                // SAFETY: `duplicato` è un handle valido posseduto dal thread bloccante, che lo tiene aperto per tutta l'attesa.
                match unsafe { WaitForSingleObject(duplicato.as_raw_handle() as HANDLE, INFINITE) }
                {
                    WAIT_OBJECT_0 => Ok(()),
                    WAIT_FAILED => Err(io::Error::last_os_error()),
                    result => Err(io::Error::other(format!(
                        "unexpected process wait result {result}"
                    ))),
                }
            })
            .await
            .map_err(|error| io::Error::other(error.to_string()))??;
            // Il processo è uscito: `try_wait` ora legge il codice e lo
            // memorizza in `self.status`, che è dove il resto del modulo se
            // lo aspetta.
            self.try_wait()?.ok_or_else(|| {
                io::Error::other("il processo risulta terminato ma senza codice di uscita")
            })
        }
    }

    /// Copia dell'handle di processo, di proprietà del chiamante.
    fn duplicate_handle(process: HANDLE) -> io::Result<OwnedHandle> {
        // SAFETY: `GetCurrentProcess` restituisce lo pseudo-handle del processo corrente: non va chiuso e non ha precondizioni.
        let corrente = unsafe { GetCurrentProcess() };
        let mut copia: HANDLE = null_mut();
        // SAFETY: `corrente` e `process` sono handle validi e `copia` è una locale viva e scrivibile; l'handle nuovo passa a `OwnedHandle` dopo il controllo dell'esito.
        let duplicato = unsafe {
            DuplicateHandle(
                corrente,
                process,
                corrente,
                &raw mut copia,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if duplicato == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: DuplicateHandle ha appena scritto un handle valido di cui
        // siamo proprietari; OwnedHandle lo chiuderà.
        Ok(unsafe { OwnedHandle::from_raw_handle(copia as _) })
    }

    impl ProcessOwner {
        pub(super) fn new() -> io::Result<Self> {
            // SAFETY: `CreateJobObjectW` accetta attributi e nome nulli; l'esito viene controllato subito dopo.
            let raw_job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if raw_job.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `raw_job` è un handle di job appena creato e non nullo (controllato sopra): `OwnedHandle` ne prende la proprietà unica.
            let job = unsafe { OwnedHandle::from_raw_handle(raw_job) };
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `job` è un handle valido e `limits` è una locale che vive fino al ritorno della chiamata, con la dimensione passata.
            let configured = unsafe {
                SetInformationJobObject(
                    job.as_raw_handle() as HANDLE,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast::<c_void>(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if configured == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { job })
        }

        pub(super) fn contains_process(&self, process: HANDLE) -> io::Result<bool> {
            let mut assigned = 0;
            // SAFETY: `process` e `self.job` sono handle validi e `assigned` è una locale viva e scrivibile.
            let queried = unsafe {
                IsProcessInJob(process, self.job.as_raw_handle() as HANDLE, &mut assigned)
            };
            if queried == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(assigned != 0)
        }

        pub(super) fn terminate(&self) -> io::Result<()> {
            // SAFETY: `self.job` è un handle di job valido posseduto da `self`.
            let terminated = unsafe { TerminateJobObject(self.job.as_raw_handle() as HANDLE, 1) };
            if terminated == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        pub(super) fn peak_job_memory_bytes(&self) -> Option<u64> {
            use std::mem::size_of;
            use std::ptr::null_mut;
            use windows_sys::Win32::Foundation::HANDLE;
            use windows_sys::Win32::System::JobObjects::{
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                QueryInformationJobObject,
            };
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            // SAFETY: `self.job` è un handle valido e `info` è una locale viva, scrivibile e della dimensione passata.
            let ok = unsafe {
                QueryInformationJobObject(
                    self.job.as_raw_handle() as HANDLE,
                    JobObjectExtendedLimitInformation,
                    (&raw mut info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    null_mut(),
                )
            };
            if ok == 0 {
                return None;
            }
            Some(info.PeakJobMemoryUsed as u64)
        }
    }

    pub(super) fn spawn(command: ServerCommand) -> io::Result<SpawnedProcess> {
        let owner = ProcessOwner::new()?;
        let (stdin, child_stdin) = create_pipe(false)?;
        let (stdout, child_stdout) = create_pipe(true)?;
        let (stderr, child_stderr) = create_pipe(true)?;
        let inherited_handles = [
            child_stdin.as_raw_handle() as HANDLE,
            child_stdout.as_raw_handle() as HANDLE,
            child_stderr.as_raw_handle() as HANDLE,
        ];
        let job_handles = [owner.job.as_raw_handle() as HANDLE];
        let mut attributes = AttributeList::new(2)?;
        attributes.update(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            &inherited_handles,
        )?;
        // PROC_THREAD_ATTRIBUTE_JOB_LIST establishes the native-spawn baseline:
        // Windows 10 and Windows Server 2016 or newer.
        attributes.update(PROC_THREAD_ATTRIBUTE_JOB_LIST as usize, &job_handles)?;

        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = inherited_handles[0];
        startup.StartupInfo.hStdOutput = inherited_handles[1];
        startup.StartupInfo.hStdError = inherited_handles[2];
        startup.lpAttributeList = attributes.as_mut_ptr();

        let mut command_line = make_command_line(&command.program, &command.args)?;
        let effective_environment = make_environment(command.env)?;
        let resolved_program = resolve_executable(
            &command.program,
            command.cwd.as_deref(),
            &effective_environment,
        )?;
        let application_name = nul_terminated(
            resolved_program.as_os_str(),
            "resolved server executable path",
        )?;
        let mut environment = effective_environment.into_block();
        let current_directory = command
            .cwd
            .as_deref()
            .map(|path| nul_terminated(path.as_os_str(), "server working directory"))
            .transpose()?;
        let current_directory_ptr = current_directory
            .as_ref()
            .map_or(null(), |directory| directory.as_ptr());
        let mut process_information = PROCESS_INFORMATION::default();
        // SAFETY: tutti i puntatori (applicazione, riga di comando, ambiente, directory, `startup` con la lista degli attributi, `process_information`) puntano a locali vive fino al ritorno di `CreateProcessW`; riga di comando e ambiente sono buffer mutabili posseduti da questa funzione.
        let created = unsafe {
            CreateProcessW(
                application_name.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                environment.as_mut_ptr().cast(),
                current_directory_ptr,
                &raw const startup.StartupInfo,
                &mut process_information,
            )
        };
        if created == 0 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: `hProcess` è un handle valido e non condiviso, perché `created` è diverso da zero: `OwnedHandle` ne prende la proprietà.
        let process = unsafe { OwnedHandle::from_raw_handle(process_information.hProcess) };
        // SAFETY: `hThread` è un handle valido e non condiviso, perché `created` è diverso da zero: `OwnedHandle` ne prende la proprietà.
        let _thread = unsafe { OwnedHandle::from_raw_handle(process_information.hThread) };
        if !owner.contains_process(process.as_raw_handle() as HANDLE)? {
            return Err(io::Error::other(
                "server was not created inside its kill-on-close Job",
            ));
        }
        drop(child_stdin);
        drop(child_stdout);
        drop(child_stderr);

        Ok(SpawnedProcess {
            child: Child {
                process,
                process_id: process_information.dwProcessId,
                status: None,
            },
            owner,
            stdin: tokio::fs::File::from_std(StdFile::from(stdin)),
            stdout: tokio::fs::File::from_std(StdFile::from(stdout)),
            stderr: tokio::fs::File::from_std(StdFile::from(stderr)),
        })
    }

    fn create_pipe(parent_reads: bool) -> io::Result<(OwnedHandle, OwnedHandle)> {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        let mut read = null_mut();
        let mut write = null_mut();
        // SAFETY: `read` e `write` sono locali vive e scrivibili e `attributes` è una struttura locale viva.
        let created = unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) };
        if created == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `read` è un handle appena creato da `CreatePipe` (`created` è diverso da zero): `OwnedHandle` ne prende la proprietà.
        let read = unsafe { OwnedHandle::from_raw_handle(read) };
        // SAFETY: `write` è un handle appena creato da `CreatePipe` (`created` è diverso da zero): `OwnedHandle` ne prende la proprietà.
        let write = unsafe { OwnedHandle::from_raw_handle(write) };
        let (parent, child) = if parent_reads {
            (read, write)
        } else {
            (write, read)
        };
        // SAFETY: `parent` è un handle valido posseduto da questa funzione.
        let protected = unsafe {
            SetHandleInformation(parent.as_raw_handle() as HANDLE, HANDLE_FLAG_INHERIT, 0)
        };
        if protected == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((parent, child))
    }

    pub(super) fn make_command_line(program: &Path, args: &[OsString]) -> io::Result<Vec<u16>> {
        let mut command_line = Vec::new();
        append_argument(&mut command_line, program.as_os_str(), true)?;
        for argument in args {
            command_line.push(' ' as u16);
            append_argument(&mut command_line, argument, false)?;
        }
        command_line.push(0);
        Ok(command_line)
    }

    fn append_argument(
        command_line: &mut Vec<u16>,
        argument: &OsStr,
        force_quotes: bool,
    ) -> io::Result<()> {
        let argument = argument.encode_wide().collect::<Vec<_>>();
        if argument.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "server argument contains a NUL character",
            ));
        }
        let quote = force_quotes
            || argument.is_empty()
            || argument
                .iter()
                .any(|unit| *unit == ' ' as u16 || *unit == '\t' as u16);
        if quote {
            command_line.push('"' as u16);
        }
        let mut backslashes = 0;
        for unit in argument {
            if unit == '\\' as u16 {
                backslashes += 1;
            } else {
                if unit == '"' as u16 {
                    command_line.extend(std::iter::repeat_n('\\' as u16, backslashes + 1));
                }
                backslashes = 0;
            }
            command_line.push(unit);
        }
        if quote {
            command_line.extend(std::iter::repeat_n('\\' as u16, backslashes));
            command_line.push('"' as u16);
        }
        Ok(())
    }

    fn make_environment(overrides: Vec<(OsString, OsString)>) -> io::Result<EffectiveEnvironment> {
        let mut environment = env::vars_os()
            .map(|(key, value)| {
                Ok((
                    environment_key(&key, true)?,
                    nul_free_wide(&value, "environment value")?,
                ))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let path_key = "PATH".encode_utf16().collect::<Vec<_>>();
        let mut path_changed = false;
        for (key, value) in overrides {
            let key = environment_key(&key, false)?;
            let value = nul_free_wide(&value, "environment value")?;
            path_changed |= compare_ordinal(&key, &path_key, true) == Ordering::Equal;
            environment
                .retain(|(existing, _)| compare_ordinal(existing, &key, true) != Ordering::Equal);
            environment.push((key, value));
        }
        environment.sort_by(|(left, _), (right, _)| {
            compare_ordinal(left, right, true).then_with(|| left.cmp(right))
        });

        Ok(EffectiveEnvironment {
            entries: environment,
            path_changed,
        })
    }

    impl EffectiveEnvironment {
        fn value(&self, key: &str) -> Option<OsString> {
            let key = key.encode_utf16().collect::<Vec<_>>();
            self.entries
                .iter()
                .find(|(existing, _)| compare_ordinal(existing, &key, true) == Ordering::Equal)
                .map(|(_, value)| OsString::from_wide(value))
        }

        fn into_block(self) -> Vec<u16> {
            let environment = self.entries;
            let mut block = Vec::new();
            if environment.is_empty() {
                block.push(0);
            }
            for (key, value) in environment {
                block.extend(key);
                block.push('=' as u16);
                block.extend(value);
                block.push(0);
            }
            block.push(0);
            block
        }
    }

    // This follows std::process::Command's Windows resolver: an explicitly changed
    // child PATH is searched first, followed by the application and system
    // directories, then the parent PATH. PATHEXT is intentionally not consulted;
    // only `.exe` is appended to an extensionless name. Unlike a null
    // lpApplicationName search, the result is made absolute before CreateProcessW.
    #[allow(clippy::indexing_slicing)] // len() >= 4 verificato nella condizione
    fn resolve_executable(
        program: &Path,
        child_cwd: Option<&Path>,
        environment: &EffectiveEnvironment,
    ) -> io::Result<PathBuf> {
        let program_name = program.as_os_str();
        let program_wide = nul_free_wide(program_name, "server executable path")?;
        if program_wide.is_empty() || program.file_name().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "server executable path has no file name",
            ));
        }
        let has_exe_suffix = program_wide.len() >= 4
            && matches!(
                &program_wide[program_wide.len() - 4..],
                [46, 101 | 69, 120 | 88, 101 | 69]
            );
        let is_file_name = program.file_name() == Some(program_name);
        let child_base = child_cwd
            .map(absolute_base)
            .transpose()?
            .unwrap_or(env::current_dir()?);

        if !is_file_name {
            if has_exe_suffix {
                return absolute_application_path(program, &child_base);
            }
            let mut with_exe = program.as_os_str().to_owned();
            with_exe.push(".exe");
            if let Some(path) = program_exists(&child_base.join(with_exe)) {
                return Ok(path);
            }
            return absolute_application_path(program, &child_base);
        }

        let has_extension = program_wide.contains(&('.' as u16));
        let mut executable_name = program.to_path_buf();
        if !has_extension {
            executable_name.set_extension("exe");
        }

        if environment.path_changed
            && let Some(paths) = environment.value("PATH")
            && let Some(path) = search_path_list(&paths, &executable_name, &child_base)
        {
            return Ok(path);
        }
        if let Ok(application) = env::current_exe()
            && let Some(directory) = application.parent()
            && let Some(path) = program_exists(&directory.join(&executable_name))
        {
            return Ok(path);
        }
        if let Some(directory) = windows_directory(GetSystemDirectoryW)
            && let Some(path) = program_exists(&directory.join(&executable_name))
        {
            return Ok(path);
        }
        if let Some(directory) = windows_directory(GetWindowsDirectoryW)
            && let Some(path) = program_exists(&directory.join(&executable_name))
        {
            return Ok(path);
        }
        if let Some(paths) = env::var_os("PATH")
            && let Some(path) = search_path_list(&paths, &executable_name, &env::current_dir()?)
        {
            return Ok(path);
        }

        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "server executable was not found",
        ))
    }

    fn absolute_base(path: &Path) -> io::Result<PathBuf> {
        if path.is_absolute() {
            Ok(path.to_owned())
        } else {
            Ok(env::current_dir()?.join(path))
        }
    }

    fn absolute_application_path(path: &Path, base: &Path) -> io::Result<PathBuf> {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            base.join(path)
        };
        Ok(std::fs::canonicalize(&path).unwrap_or(path))
    }

    fn search_path_list(paths: &OsStr, name: &Path, relative_base: &Path) -> Option<PathBuf> {
        env::split_paths(paths)
            .filter(|directory| !directory.as_os_str().is_empty())
            .find_map(|directory| {
                let directory = if directory.is_absolute() {
                    directory
                } else {
                    relative_base.join(directory)
                };
                program_exists(&directory.join(name))
            })
    }

    fn program_exists(path: &Path) -> Option<PathBuf> {
        if !std::fs::metadata(path).ok()?.is_file() {
            return None;
        }
        std::fs::canonicalize(path)
            .ok()
            .or_else(|| Some(path.to_owned()))
    }

    fn windows_directory(
        get_directory: unsafe extern "system" fn(*mut u16, u32) -> u32,
    ) -> Option<PathBuf> {
        let mut buffer = vec![0_u16; 260];
        loop {
            // SAFETY: `get_directory` è una funzione di sistema che scrive al più `buffer.len()` unità nel buffer, vivo e scrivibile.
            let length =
                unsafe { get_directory(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
            if length == 0 {
                return None;
            }
            if length < buffer.len() {
                buffer.truncate(length);
                return Some(PathBuf::from(OsString::from_wide(&buffer)));
            }
            buffer.resize(length + 1, 0);
        }
    }

    #[allow(clippy::indexing_slicing)] // key.len() > 1 verificato nella condizione
    pub(super) fn environment_key(
        key: &OsStr,
        allow_inherited_pseudo_key: bool,
    ) -> io::Result<Vec<u16>> {
        let key = nul_free_wide(key, "environment key")?;
        let equals = '=' as u16;
        let is_inherited_pseudo_key = allow_inherited_pseudo_key
            && key.first() == Some(&equals)
            && key.len() > 1
            && !key[1..].contains(&equals);
        if key.is_empty() || (key.contains(&equals) && !is_inherited_pseudo_key) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "environment key is empty or contains '='",
            ));
        }
        Ok(key)
    }

    fn nul_free_wide(value: &OsStr, label: &str) -> io::Result<Vec<u16>> {
        let value = value.encode_wide().collect::<Vec<_>>();
        if value.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{label} contains a NUL character"),
            ));
        }
        Ok(value)
    }

    fn nul_terminated(value: &OsStr, label: &str) -> io::Result<Vec<u16>> {
        let mut value = nul_free_wide(value, label)?;
        value.push(0);
        Ok(value)
    }

    fn compare_ordinal(left: &[u16], right: &[u16], ignore_case: bool) -> Ordering {
        let Ok(left_len) = i32::try_from(left.len()) else {
            return left.cmp(right);
        };
        let Ok(right_len) = i32::try_from(right.len()) else {
            return left.cmp(right);
        };
        // SAFETY: `left` e `right` sono slice vive e le loro lunghezze sono già state verificate entro `i32::MAX`.
        match unsafe {
            CompareStringOrdinal(
                left.as_ptr(),
                left_len,
                right.as_ptr(),
                right_len,
                i32::from(ignore_case),
            )
        } {
            CSTR_LESS_THAN => Ordering::Less,
            CSTR_EQUAL => Ordering::Equal,
            CSTR_GREATER_THAN => Ordering::Greater,
            _ => left.cmp(right),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod platform {
    use std::{
        fs,
        io::{self, Read},
        os::unix::{fs::DirBuilderExt, net::UnixListener, process::CommandExt},
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        thread,
        time::{Duration, Instant},
    };

    use crate::process_watchdog::{Readiness, WatchdogCommand};

    use super::ServerCommand;

    const READY_TIMEOUT: Duration = Duration::from_secs(5);
    const READY_POLL_INTERVAL: Duration = Duration::from_millis(5);
    const MAX_READY_BYTES: u64 = 16 * 1024;
    static ENDPOINT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    pub(super) struct PreparedOwner {
        endpoint: ReadinessEndpoint,
    }

    pub(super) struct ProcessOwner {
        process_group: i32,
        server_pid: u32,
    }

    struct ReadinessEndpoint {
        listener: UnixListener,
        directory: PathBuf,
        socket: PathBuf,
    }

    pub(super) fn command(
        server: ServerCommand,
    ) -> io::Result<(tokio::process::Command, PreparedOwner)> {
        let endpoint = ReadinessEndpoint::new()?;
        let helper = helper_path()?;
        let configuration = WatchdogCommand {
            program: server.program.into_os_string(),
            args: server.args,
            cwd: server.cwd,
            env: server.env,
            parent_pid: std::process::id(),
            readiness_socket: endpoint.socket.clone(),
        };
        let encoded = configuration
            .to_environment()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let mut command = tokio::process::Command::new(helper);
        command.envs(encoded).kill_on_drop(true);
        command.as_std_mut().process_group(0);
        Ok((command, PreparedOwner { endpoint }))
    }

    impl PreparedOwner {
        pub(super) fn attach(self, helper_pid: u32) -> io::Result<ProcessOwner> {
            let process_group = i32::try_from(helper_pid)
                .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
            let owner = ProcessOwner {
                process_group,
                server_pid: helper_pid,
            };
            // SAFETY: `getpgid` non ha precondizioni sulla memoria: un pid inesistente dà -1.
            if unsafe { libc::getpgid(process_group) } != process_group {
                let _ = owner.terminate();
                return Err(io::Error::other(
                    "process watchdog did not start as process-group leader",
                ));
            }
            let readiness = match self.endpoint.receive() {
                Ok(readiness) => readiness,
                Err(error) => {
                    let _ = owner.terminate();
                    return Err(error);
                }
            };
            match readiness {
                Readiness::Ready { server_pid } => {
                    let server = i32::try_from(server_pid)
                        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
                    // SAFETY: `getpgid` non ha precondizioni sulla memoria: un pid inesistente dà -1.
                    let server_group = unsafe { libc::getpgid(server) };
                    let server_exited = server_group == -1
                        && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
                    if server_group != process_group && !server_exited {
                        let _ = owner.terminate();
                        return Err(io::Error::other(
                            "language server escaped the watchdog process group",
                        ));
                    }
                    Ok(ProcessOwner {
                        process_group,
                        server_pid,
                    })
                }
                Readiness::Error(message) => {
                    let _ = owner.terminate();
                    Err(io::Error::other(format!(
                        "process watchdog could not start language server: {message}"
                    )))
                }
            }
        }
    }

    impl ProcessOwner {
        pub(super) fn process_id(&self) -> u32 {
            self.server_pid
        }

        pub(super) fn peak_job_memory_bytes(&self) -> Option<u64> {
            None
        }

        pub(super) fn terminate(&self) -> io::Result<()> {
            // SAFETY: `kill` riceve solo interi: `-self.process_group` indica il gruppo, non un puntatore.
            if unsafe { libc::kill(-self.process_group, libc::SIGKILL) } == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            Err(error)
        }
    }

    impl ReadinessEndpoint {
        fn new() -> io::Result<Self> {
            for _ in 0..128 {
                let sequence = ENDPOINT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let directory =
                    std::env::temp_dir().join(format!("lume-wd-{}-{sequence}", std::process::id()));
                match fs::DirBuilder::new().mode(0o700).create(&directory) {
                    Ok(()) => {
                        let socket = directory.join("ready.sock");
                        match UnixListener::bind(&socket) {
                            Ok(listener) => {
                                listener.set_nonblocking(true)?;
                                return Ok(Self {
                                    listener,
                                    directory,
                                    socket,
                                });
                            }
                            Err(error) => {
                                let _ = fs::remove_dir(&directory);
                                return Err(error);
                            }
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "could not allocate private watchdog readiness endpoint",
            ))
        }

        fn receive(&self) -> io::Result<Readiness> {
            let deadline = Instant::now() + READY_TIMEOUT;
            loop {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_read_timeout(Some(READY_TIMEOUT))?;
                        let mut bytes = Vec::new();
                        stream.take(MAX_READY_BYTES).read_to_end(&mut bytes)?;
                        return Readiness::decode(&bytes)
                            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "process watchdog readiness timed out",
                            ));
                        }
                        thread::sleep(READY_POLL_INTERVAL);
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }

    impl Drop for ReadinessEndpoint {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.socket);
            let _ = fs::remove_dir(&self.directory);
        }
    }

    fn helper_path() -> io::Result<PathBuf> {
        let current = std::env::current_exe()?;
        let parent = current
            .parent()
            .ok_or_else(|| io::Error::other("current executable has no parent directory"))?;
        let mut candidates = vec![parent.join("lume-process-watchdog")];
        if parent.file_name().is_some_and(|name| name == "deps")
            && let Some(target_dir) = parent.parent()
        {
            candidates.push(target_dir.join("lume-process-watchdog"));
        }
        candidates
            .into_iter()
            .find_map(|candidate| executable_file(&candidate))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "lume-process-watchdog is not installed beside the application",
                )
            })
    }

    fn executable_file(path: &Path) -> Option<PathBuf> {
        fs::metadata(path)
            .ok()
            .filter(|metadata| metadata.is_file())?;
        fs::canonicalize(path).ok()
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod platform {
    use std::io;

    use super::ServerCommand;

    pub(super) struct PreparedOwner;
    pub(super) struct ProcessOwner(u32);

    pub(super) fn command(
        server: ServerCommand,
    ) -> io::Result<(tokio::process::Command, PreparedOwner)> {
        let mut command = tokio::process::Command::new(server.program);
        command
            .args(server.args)
            .envs(server.env)
            .kill_on_drop(true);
        if let Some(cwd) = server.cwd {
            command.current_dir(cwd);
        }
        Ok((command, PreparedOwner))
    }

    impl PreparedOwner {
        pub(super) fn attach(self, process_id: u32) -> io::Result<ProcessOwner> {
            Ok(ProcessOwner(process_id))
        }
    }

    impl ProcessOwner {
        pub(super) fn process_id(&self) -> u32 {
            self.0
        }

        pub(super) fn peak_job_memory_bytes(&self) -> Option<u64> {
            None
        }

        pub(super) fn terminate(&self) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(windows)]
use platform::Child as PlatformChild;
use platform::ProcessOwner;

/// Stato condiviso tra chi consegna i figli e il worker della generazione
/// corrente. `sender` è `Some` solo finché un worker è vivo e può ancora
/// ricevere: è il worker stesso ad azzerarlo, sotto questo lock, quando esce.
struct ReaperState {
    sender: Option<Sender<PlatformChild>>,
    generation: u64,
    // Handle dell'unico worker più recente. Un worker a lista vuota azzera
    // `sender` prima di uscire; il prossimo handoff lo unisce prima di creare
    // la generazione successiva. I test possono unirlo subito via ack.
    worker: Option<(u64, std::thread::JoinHandle<()>)>,
    // Spawn fallito dopo start_kill NON è una raccolta. Questi handle
    // restano posseduti e passano al prossimo worker nato da un handoff.
    retained_handoffs: Vec<PlatformChild>,
    // Wait non confermato: handle+errore NON contano come raccolti e NON
    // vengono rispediti alla cieca (PID/status possono essere incerti).
    wait_failures: Vec<(PlatformChild, io::Error)>,
}

struct ReaperShared {
    state: Mutex<ReaperState>,
    state_changed: std::sync::Condvar,
    #[cfg(test)]
    lifecycle: ReaperLifecycle,
}

/// Osservabilità del solo test: quali generazioni sono uscite (e quanti
/// figli hanno raccolto) e il gancio sul confine lista-vuota/uscita. Si
/// aspetta su una `Condvar`, mai a polling.
#[cfg(test)]
#[derive(Default)]
struct ReaperLifecycle {
    stopped: Mutex<Vec<(u64, usize, usize)>>,
    joined: Mutex<Vec<u64>>,
    stopped_changed: std::sync::Condvar,
    before_exit_lock: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// Il reaper dei figli dei language server, **riavviabile**.
///
/// ATTENZIONE: Prima era un thread permanente (`OnceLock` su un `Sender`): nato al
/// primo server, vivo fino alla chiusura dell'app anche con zero figli da
/// raccogliere. Ora il worker esce appena la sua lista si svuota e il
/// passaggio di mano successivo ne fa nascere uno nuovo (generazione
/// successiva): da inattivi non resta nessun thread.
#[derive(Clone)]
struct ReaperSupervisor {
    shared: Arc<ReaperShared>,
}

impl ReaperSupervisor {
    fn new() -> Self {
        Self {
            shared: Arc::new(ReaperShared {
                state: Mutex::new(ReaperState {
                    sender: None,
                    generation: 0,
                    worker: None,
                    retained_handoffs: Vec::new(),
                    wait_failures: Vec::new(),
                }),
                state_changed: std::sync::Condvar::new(),
                #[cfg(test)]
                lifecycle: ReaperLifecycle::default(),
            }),
        }
    }

    /// Consegna un figlio da uccidere e raccogliere; restituisce la
    /// generazione del worker che lo ha preso in carico.
    ///
    /// Il lock è tenuto solo per il tempo dell'invio (o della nascita del
    /// worker): mai durante il polling di `try_wait`, che sta nel worker.
    fn handoff(&self, child: PlatformChild) -> io::Result<u64> {
        self.handoff_with_builder(
            child,
            std::thread::Builder::new().name("lume-lsp-process-reaper".into()),
        )
    }

    // Stesso spawn nativo; permette al test di usare attributi stack che
    // l'OS rifiuta, non uno spawner/child mock o errore sintetico.
    fn handoff_with_builder(
        &self,
        mut child: PlatformChild,
        builder: std::thread::Builder,
    ) -> io::Result<u64> {
        // Kill subito, prima di toccare il lock: se il worker non potesse
        // nascere il figlio sarebbe comunque già stato terminato.
        let _ = child.start_kill();
        let mut state = lock(&self.shared.state);
        if let Some(sender) = state.sender.as_ref() {
            match sender.send(child) {
                Ok(()) => return Ok(state.generation),
                // Worker morto (panico) senza aver azzerato il mittente:
                // il figlio torna indietro e ne nasce uno nuovo.
                Err(mpsc::SendError(returned)) => child = returned,
            }
        }
        state.sender = None;
        if let Some((worker_generation, worker)) = state.worker.take() {
            if worker.join().is_err() {
                state.retained_handoffs.push(child);
                return Err(io::Error::other(
                    "previous LSP process reaper worker panicked before join",
                ));
            }
            #[cfg(test)]
            self.shared.lifecycle.report_joined(worker_generation);
            #[cfg(not(test))]
            let _ = worker_generation;
        }
        state.retained_handoffs.push(child);
        self.start_worker_from_retained(&mut state, builder)
    }

    fn start_worker_from_retained(
        &self,
        state: &mut ReaperState,
        builder: std::thread::Builder,
    ) -> io::Result<u64> {
        let generation = state
            .generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("LSP process reaper generation exhausted"))?;
        let (sender, receiver) = mpsc::channel::<PlatformChild>();
        // Prima dello spawn il receiver è posseduto da noi: in caso di errore
        // si ripristinano tutti gli handle, in ordine, nel supervisor.
        for pending in state.retained_handoffs.drain(..) {
            let _ = sender.send(pending);
        }
        // Il parent conserva una copia del receiver finché spawn restituisce
        // successo: se la closure viene distrutta per errore non perde figli.
        let receiver = Arc::new(Mutex::new(receiver));
        let worker_receiver = Arc::clone(&receiver);
        let shared = Arc::clone(&self.shared);
        let worker = match builder.spawn(move || {
            let receiver = lock(&worker_receiver);
            run_reaper(&shared, &receiver, generation);
        }) {
            Ok(worker) => worker,
            Err(error) => {
                state.retained_handoffs.extend(lock(&receiver).try_iter());
                return Err(error);
            }
        };
        state.generation = generation;
        state.sender = Some(sender);
        state.worker = Some((generation, worker));
        Ok(generation)
    }

    /// Attende che ogni figlio consegnato venga raccolto, poi unisce il worker
    /// finale. Va chiamato quando l'app non può più creare sessioni LSP.
    fn drain(&self, timeout: Duration) -> io::Result<usize> {
        let started = std::time::Instant::now();
        let mut joined = 0usize;
        loop {
            let mut state = lock(&self.shared.state);
            if state.sender.is_some() {
                let remaining = timeout.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "LSP process reaper drain exceeded its deadline",
                    ));
                }
                let (next, result) = self
                    .shared
                    .state_changed
                    .wait_timeout_while(state, remaining, |state| state.sender.is_some())
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state = next;
                if result.timed_out() && state.sender.is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "LSP process reaper drain exceeded its deadline",
                    ));
                }
                drop(state);
                continue;
            }

            if let Some((generation, worker)) = state.worker.take() {
                drop(state);
                worker.join().map_err(|_| {
                    io::Error::other(format!(
                        "LSP process reaper worker {generation} panicked before drain join"
                    ))
                })?;
                joined = joined.saturating_add(1);
                #[cfg(test)]
                self.shared.lifecycle.report_joined(generation);
                continue;
            }

            if !state.retained_handoffs.is_empty() {
                self.start_worker_from_retained(
                    &mut state,
                    std::thread::Builder::new().name("lume-lsp-process-reaper".into()),
                )?;
                drop(state);
                continue;
            }

            if !state.wait_failures.is_empty() {
                return Err(io::Error::other(format!(
                    "LSP process reaper drain has {} unconfirmed child waits",
                    state.wait_failures.len()
                )));
            }
            return Ok(joined);
        }
    }
}

/// Il worker di una generazione: possiede ricevitore e lista dei figli.
/// Polling con `recv_timeout` solo mentre ci sono figli da aspettare; a
/// lista vuota esce, dopo aver drenato sotto il lock ogni consegna già in
/// coda (altrimenti una consegna arrivata tra «lista vuota» e «azzero il
/// mittente» andrebbe persa).
#[cfg_attr(not(test), allow(unused_assignments, unused_variables))]
fn run_reaper(shared: &ReaperShared, receiver: &mpsc::Receiver<PlatformChild>, generation: u64) {
    let mut children: Vec<PlatformChild> = Vec::new();
    let mut reaped = 0usize;
    let mut failed = 0usize;
    loop {
        while let Ok(child) = receiver.try_recv() {
            children.push(child);
        }
        let (collected, failures) = poll_reaper_children(&mut children, |child| child.try_wait());
        reaped = reaped.saturating_add(collected);
        failed = failed.saturating_add(failures.len());
        if !failures.is_empty() {
            lock(&shared.state).wait_failures.extend(failures);
        }

        if children.is_empty() {
            #[cfg(test)]
            shared.lifecycle.run_before_exit_lock_hook();
            let mut state = lock(&shared.state);
            while let Ok(child) = receiver.try_recv() {
                children.push(child);
            }
            if children.is_empty() {
                state.sender = None;
                drop(state);
                shared.state_changed.notify_all();
                #[cfg(test)]
                shared.lifecycle.report_stopped(generation, reaped, failed);
                return;
            }
            continue;
        }

        match receiver.recv_timeout(REAPER_POLL_INTERVAL) {
            Ok(child) => children.push(child),
            Err(RecvTimeoutError::Timeout) => {}
            // Il mittente vive nello stato condiviso finché il worker non lo
            // azzera: non succede. Se succedesse, si finisce di raccogliere
            // con la stessa cadenza invece di girare a vuoto.
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(REAPER_POLL_INTERVAL),
        }
    }
}

// Seam di fault injection del singolo poll, con child OS reali nei test.
// Il caller production usa esclusivamente PlatformChild::try_wait.
fn poll_reaper_children(
    children: &mut Vec<PlatformChild>,
    mut poll: impl FnMut(&mut PlatformChild) -> io::Result<Option<ExitStatus>>,
) -> (usize, Vec<(PlatformChild, io::Error)>) {
    let mut collected = 0;
    let mut failures = Vec::new();
    let mut index = 0;
    while index < children.len() {
        let Some(child) = children.get_mut(index) else {
            break;
        };
        match poll(child) {
            Ok(None) => index += 1,
            Ok(Some(_)) => {
                children.swap_remove(index);
                collected += 1;
            }
            Err(error) => failures.push((children.swap_remove(index), error)),
        }
    }
    (collected, failures)
}

#[cfg(test)]
impl ReaperLifecycle {
    fn run_before_exit_lock_hook(&self) {
        let hook = lock(&self.before_exit_lock).take();
        if let Some(hook) = hook {
            hook();
        }
    }

    fn report_stopped(&self, generation: u64, reaped: usize, failed: usize) {
        lock(&self.stopped).push((generation, reaped, failed));
        self.stopped_changed.notify_all();
    }

    fn report_joined(&self, generation: u64) {
        lock(&self.joined).push(generation);
    }
}

#[cfg(test)]
impl ReaperSupervisor {
    fn new_for_test() -> Self {
        Self::new()
    }

    /// Gancio una tantum eseguito dal worker nel punto esatto del confine:
    /// lista vuota, lock non ancora preso.
    fn set_before_exit_lock_hook(&self, hook: impl FnOnce() + Send + 'static) {
        *lock(&self.shared.lifecycle.before_exit_lock) = Some(Box::new(hook));
    }

    /// Attende l'ack del worker senza polling, ma non unisce il JoinHandle.
    fn wait_reported(&self, generation: u64, timeout: Duration) -> io::Result<(usize, usize)> {
        let lifecycle = &self.shared.lifecycle;
        let guard = lock(&lifecycle.stopped);
        let (guard, _) = lifecycle
            .stopped_changed
            .wait_timeout_while(guard, timeout, |stopped| {
                !stopped.iter().any(|(stopped, _, _)| *stopped == generation)
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .iter()
            .find(|(stopped, _, _)| *stopped == generation)
            .map(|(_, reaped, failed)| (*reaped, *failed))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("il worker {generation} non è uscito entro il tempo"),
                )
            })
    }

    /// Attende l'ack e poi fa join sul thread OS vero prima di confermare la
    /// generazione come terminata.
    fn wait_stopped(&self, generation: u64, timeout: Duration) -> io::Result<usize> {
        let (reaped, failed) = self.wait_reported(generation, timeout)?;
        let joined_now = if let Some(worker) = {
            let mut state = lock(&self.shared.state);
            match state.worker.as_ref() {
                Some((worker_generation, _)) if *worker_generation == generation => {
                    state.worker.take().map(|(_, worker)| worker)
                }
                _ => None,
            }
        } {
            worker.join().map_err(|_| {
                io::Error::other(format!(
                    "worker {generation} è andato in panic prima del join"
                ))
            })?;
            self.shared.lifecycle.report_joined(generation);
            true
        } else if failed == 0 {
            lock(&self.shared.lifecycle.joined).contains(&generation)
        } else {
            false
        };
        if failed == 0 && !joined_now {
            return Err(io::Error::other(format!(
                "worker {generation} fermo senza JoinHandle unito"
            )));
        }
        if failed == 0 {
            Ok(reaped)
        } else {
            Err(io::Error::other(format!(
                "worker {generation}: {failed} wait falliti, {reaped} raccolti; ownership residua da verificare"
            )))
        }
    }
}

static DETACHED_REAPER: OnceLock<ReaperSupervisor> = OnceLock::new();

fn handoff_reap(child: PlatformChild) {
    let _ = DETACHED_REAPER
        .get_or_init(ReaperSupervisor::new)
        .handoff(child);
}

/// Draina e unisce il reaper globale dopo che l'applicazione ha fermato gli
/// attori LSP. Non deve essere invocata mentre possono nascere nuove sessioni.
pub fn drain_process_reaper(timeout: Duration) -> io::Result<usize> {
    DETACHED_REAPER
        .get()
        .map_or(Ok(0), |supervisor| supervisor.drain(timeout))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSource {
    Bundled,
    Managed,
    SystemPath,
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(OsString, OsString)>,
    pub source: CommandSource,
}

#[cfg(not(windows))]
struct SpawnGuard {
    child: Option<PlatformChild>,
    owner: Option<ProcessOwner>,
}

#[cfg(not(windows))]
impl SpawnGuard {
    fn new(child: PlatformChild) -> Self {
        Self {
            child: Some(child),
            owner: None,
        }
    }

    fn child_mut(&mut self) -> Result<&mut PlatformChild, ClientError> {
        self.child.as_mut().ok_or_else(|| {
            ClientError::Io("la guardia di avvio non possiede più il processo".into())
        })
    }

    fn set_owner(&mut self, owner: ProcessOwner) {
        self.owner = Some(owner);
    }

    fn disarm(mut self) -> Result<(PlatformChild, ProcessOwner), ClientError> {
        let owner = self.owner.take().ok_or_else(|| {
            ClientError::Io("la guardia di avvio non possiede il gruppo di processi".into())
        })?;
        match self.child.take() {
            Some(child) => Ok((child, owner)),
            None => {
                let _ = owner.terminate();
                Err(ClientError::Io("il processo è già stato consumato".into()))
            }
        }
    }
}

#[cfg(not(windows))]
impl Drop for SpawnGuard {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.as_ref() {
            let _ = owner.terminate();
        }
        if let Some(child) = self.child.take() {
            handoff_reap(child);
        }
    }
}

#[derive(Default)]
struct StderrRing {
    bytes: VecDeque<u8>,
}

impl StderrRing {
    #[allow(clippy::indexing_slicing)] // bytes.len() >= STDERR_CAPACITY verificato
    fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= STDERR_CAPACITY {
            self.bytes.clear();
            self.bytes
                .extend(bytes[bytes.len() - STDERR_CAPACITY..].iter().copied());
            return;
        }

        let overflow = self
            .bytes
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(STDERR_CAPACITY);
        if overflow > 0 {
            self.bytes.drain(..overflow);
        }
        self.bytes.extend(bytes.iter().copied());
    }

    fn snapshot(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }
}

pub struct ServerProcess {
    child: Option<PlatformChild>,
    owner: ProcessOwner,
    process_id: u32,
    transport: ClientTransport,
    events: TransportEvents,
    stderr: Arc<Mutex<StderrRing>>,
    stderr_task: Option<JoinHandle<()>>,
}

impl ServerProcess {
    pub fn spawn(command: ServerCommand, config: TransportConfig) -> Result<Self, ClientError> {
        Self::spawn_with_trust(command, config, WorkspaceTrust::Unknown)
    }

    #[allow(clippy::indexing_slicing)] // read <= buffer.len() (contratto di AsyncRead)
    pub fn spawn_with_trust(
        command: ServerCommand,
        config: TransportConfig,
        trust: WorkspaceTrust,
    ) -> Result<Self, ClientError> {
        command
            .validate(trust)
            .map_err(|required| ClientError::TrustRequired {
                decision: required.decision,
            })?;
        #[cfg(windows)]
        let platform::SpawnedProcess {
            child,
            owner,
            stdin,
            stdout,
            stderr: mut stderr_pipe,
        } = platform::spawn(command).map_err(|error| {
            ClientError::Io(format!("could not spawn owned server process: {error}"))
        })?;

        #[cfg(not(windows))]
        let (child, owner, stdin, stdout, mut stderr_pipe) = {
            let (mut child_command, prepared_owner) =
                platform::command(command).map_err(|error| {
                    ClientError::Io(format!(
                        "could not configure server process ownership: {error}"
                    ))
                })?;
            child_command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);

            let child = child_command.spawn().map_err(|error| {
                ClientError::Io(format!("could not spawn server process: {error}"))
            })?;
            let mut guard = SpawnGuard::new(child);
            let process_id = guard
                .child_mut()?
                .id()
                .ok_or_else(|| ClientError::Io("spawned server has no process ID".into()))?;
            let owner = prepared_owner.attach(process_id).map_err(|error| {
                ClientError::Io(format!(
                    "could not assign server process ownership: {error}"
                ))
            })?;
            guard.set_owner(owner);
            let stdin = guard
                .child_mut()?
                .stdin
                .take()
                .ok_or_else(|| ClientError::Io("server stdin pipe unavailable".into()))?;
            let stdout = guard
                .child_mut()?
                .stdout
                .take()
                .ok_or_else(|| ClientError::Io("server stdout pipe unavailable".into()))?;
            let stderr_pipe = guard
                .child_mut()?
                .stderr
                .take()
                .ok_or_else(|| ClientError::Io("server stderr pipe unavailable".into()))?;

            let (child, owner) = guard.disarm()?;
            (child, owner, stdin, stdout, stderr_pipe)
        };

        #[cfg(windows)]
        let process_id = child
            .id()
            .ok_or_else(|| ClientError::Io("spawned server has no process ID".into()))?;
        #[cfg(not(windows))]
        let process_id = owner.process_id();

        let (transport, events) = ClientTransport::spawn(stdout, stdin, config);
        let stderr = Arc::new(Mutex::new(StderrRing::default()));
        let stderr_sink = Arc::clone(&stderr);
        let stderr_task = tokio::spawn(async move {
            let mut buffer = vec![0_u8; STDERR_READ_BUFFER];
            loop {
                match stderr_pipe.read(&mut buffer).await {
                    Ok(0) | Err(_) => return,
                    Ok(read) => lock(&stderr_sink).push(&buffer[..read]),
                }
            }
        });

        Ok(Self {
            child: Some(child),
            owner,
            process_id,
            transport,
            events,
            stderr,
            stderr_task: Some(stderr_task),
        })
    }

    pub fn transport(&self) -> &ClientTransport {
        &self.transport
    }

    pub fn events(&mut self) -> &mut TransportEvents {
        &mut self.events
    }

    pub fn stderr_snapshot(&self) -> Vec<u8> {
        lock(&self.stderr).snapshot()
    }

    pub fn process_id(&self) -> u32 {
        self.process_id
    }

    pub fn peak_memory_bytes(&self) -> Option<u64> {
        self.owner.peak_job_memory_bytes()
    }

    #[allow(clippy::expect_used)] // child presente finché shutdown non lo consuma (invariante)
    pub async fn shutdown(mut self, timeout: Duration) -> Result<ExitStatus, ClientError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let shutdown_result = self
            .transport
            .request::<_, Value>("shutdown", Value::Null, timeout)
            .await;
        let exit_result = self.transport.notify("exit", Value::Null);
        let mut child = self
            .child
            .take()
            .expect("live server process owns its child");
        let mut status = None;
        let mut cleanup_error = None;
        let mut ownership_terminated = false;

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Ok(result) = tokio::time::timeout(remaining, child.wait()).await {
            match result {
                Ok(exit_status) => status = Some(exit_status),
                Err(error) => {
                    cleanup_error = Some(ClientError::Io(format!(
                        "could not wait for server process: {error}"
                    )));
                }
            }
        }

        if status.is_none() {
            match self.owner.terminate() {
                Ok(()) => ownership_terminated = true,
                Err(owner_error) => {
                    if let Err(kill_error) = child.start_kill() {
                        cleanup_error.get_or_insert_with(|| {
                            ClientError::Io(format!(
                                "could not terminate server ownership ({owner_error}) or process ({kill_error})"
                            ))
                        });
                    }
                }
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if let Ok(result) = tokio::time::timeout(remaining, child.wait()).await {
                match result {
                    Ok(exit_status) => status = Some(exit_status),
                    Err(error) => {
                        cleanup_error.get_or_insert_with(|| {
                            ClientError::Io(format!("could not reap server process: {error}"))
                        });
                    }
                }
            }
        }

        if !ownership_terminated && let Err(error) = self.owner.terminate() {
            cleanup_error.get_or_insert_with(|| {
                ClientError::Io(format!("could not terminate server ownership: {error}"))
            });
        }

        if status.is_none() {
            handoff_reap(child);
        }

        if let Some(mut stderr_task) = self.stderr_task.take() {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, &mut stderr_task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    cleanup_error.get_or_insert_with(|| {
                        ClientError::Io(format!("stderr reader task failed: {error}"))
                    });
                }
                Err(_) => stderr_task.abort(),
            }
        }

        shutdown_result?;
        exit_result?;
        if let Some(error) = cleanup_error {
            return Err(error);
        }
        status.ok_or_else(|| {
            ClientError::Io("server process reaping exceeded the shutdown deadline".into())
        })
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.owner.terminate();
        if let Some(child) = self.child.take() {
            handoff_reap(child);
        }
        if let Some(stderr_task) = self.stderr_task.take() {
            stderr_task.abort();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(all(test, target_os = "linux"))]
mod unix_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    #[test]
    fn linux_owned_process_starts_and_reaps_without_an_internal_fork() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build process spawn runtime");
        runtime.block_on(async {
            let (mut command, prepared) = super::platform::command(super::ServerCommand {
                program: "true".into(),
                args: Vec::new(),
                cwd: None,
                env: Vec::new(),
                source: super::CommandSource::SystemPath,
            })
            .expect("prepare owned process");
            let mut child = command.spawn().expect("spawn owned process");
            let process_id = child.id().expect("spawned true process ID");
            let owner = prepared.attach(process_id).expect("attach process owner");
            let status = tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
                .await
                .expect("true process wait timed out")
                .expect("wait for true process");
            assert!(status.success());
            owner.terminate().expect("terminate empty process group");
        });
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use std::{ffi::OsString, path::Path};

    #[test]
    fn windows_command_line_preserves_quotes_and_trailing_backslashes() {
        let command_line = super::platform::make_command_line(
            Path::new(r"C:\Program Files\server.exe"),
            &[
                OsString::from("two words"),
                OsString::from("folder with space\\"),
                OsString::from("quote\"inside"),
                OsString::new(),
            ],
        )
        .expect("encode Windows command line");
        let encoded = String::from_utf16(&command_line[..command_line.len() - 1])
            .expect("valid test command line");

        assert_eq!(
            encoded,
            "\"C:\\Program Files\\server.exe\" \"two words\" \"folder with space\\\\\" quote\\\"inside \"\""
        );
    }

    #[test]
    fn windows_inherited_drive_current_directory_key_is_encodable() {
        let key = super::platform::environment_key(OsString::from("=C:").as_os_str(), true)
            .expect("preserve inherited drive-current-directory key");

        assert_eq!(String::from_utf16(&key).unwrap(), "=C:");
    }

    /// `wait()` restituisce il codice d'uscita vero di un processo che finisce
    /// **dopo** che l'attesa e' cominciata.
    ///
    /// Prima questa funzione faceva `try_wait` in ciclo con `sleep(10 ms)`:
    /// cento risvegli al secondo per ogni server vivo. Ora blocca su
    /// `WaitForSingleObject(INFINITE)` dentro `spawn_blocking`, quindi e' il
    /// sistema operativo a svegliare il thread.
    ///
    /// ATTENZIONE: Il test copre la **correttezza** (il codice giusto, e nessuno stallo
    /// se il processo esce mentre aspettiamo), non l'assenza di poll: qui non
    /// c'e' un canale di risvegli da contare come nel PTY. Che il poll non ci
    /// sia lo dice la funzione, che non contiene piu' alcun `sleep`.
    #[test]
    fn windows_wait_riporta_luscita_di_un_processo_che_finisce_dopo() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build process spawn runtime");
        runtime.block_on(async {
            // `timeout 1` dorme un secondo e basta; ci serve un processo che
            // sia ancora vivo quando `wait()` comincia ad aspettarlo.
            let mut spawned = super::platform::spawn(super::ServerCommand {
                program: "cmd.exe".into(),
                args: vec![
                    std::ffi::OsString::from("/c"),
                    std::ffi::OsString::from("ping -n 2 127.0.0.1 >NUL & exit 7"),
                ],
                cwd: None,
                env: Vec::new(),
                source: super::CommandSource::SystemPath,
            })
            .expect("spawn del processo di prova");

            let status =
                tokio::time::timeout(std::time::Duration::from_secs(20), spawned.child.wait())
                    .await
                    .expect("wait() non deve stallare")
                    .expect("wait() deve leggere il codice d'uscita");

            assert_eq!(
                status.code(),
                Some(7),
                "il codice d'uscita deve arrivare intatto"
            );

            // Una seconda chiamata legge lo stato memorizzato, non riaspetta.
            let ancora =
                tokio::time::timeout(std::time::Duration::from_secs(2), spawned.child.wait())
                    .await
                    .expect("la seconda wait() deve tornare subito")
                    .expect("stato gia' noto");
            assert_eq!(ancora.code(), Some(7));

            spawned.owner.terminate().expect("chiusura del Job");
        });
    }

    #[test]
    fn windows_exact_job_query_distinguishes_the_private_owner() {
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let owner = super::platform::ProcessOwner::new().expect("create private test Job");
        // SAFETY: `GetCurrentProcess` restituisce lo pseudo-handle del processo corrente e non ha precondizioni.
        let current_process = unsafe { GetCurrentProcess() };

        assert!(
            !owner
                .contains_process(current_process)
                .expect("query exact private Job membership"),
            "membership in an outer runner Job must not satisfy the private Job query"
        );
    }
}

#[cfg(test)]
mod reaper_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::{sync::mpsc, time::Duration};

    use super::{DETACHED_REAPER, PlatformChild, ReaperSupervisor};

    /// Tetto di un'asserzione, non una cadenza del supervisore: il reaper
    /// non fa polling per farsi osservare.
    const TEST_TIMEOUT: Duration = Duration::from_secs(20);

    /// Figlio vero che esce subito da solo.
    fn short_lived_child() -> PlatformChild {
        spawn_child("exit 0").0
    }

    /// Figlio vero che resta vivo finche' non lo si uccide. Il secondo
    /// valore tiene in vita Job e pipe: senza, la chiusura del Job lo
    /// ucciderebbe al posto del reaper.
    fn long_lived_child() -> (PlatformChild, Box<dyn std::any::Any>) {
        spawn_child("ping -n 60 127.0.0.1 >NUL")
    }

    #[cfg(windows)]
    fn spawn_child(script: &str) -> (PlatformChild, Box<dyn std::any::Any>) {
        let spawned = super::platform::spawn(super::ServerCommand {
            program: "cmd.exe".into(),
            args: vec!["/c".into(), script.into()],
            cwd: None,
            env: Vec::new(),
            source: super::CommandSource::SystemPath,
        })
        .unwrap();
        let super::platform::SpawnedProcess {
            child,
            owner,
            stdin,
            stdout,
            stderr,
        } = spawned;
        (child, Box::new((owner, stdin, stdout, stderr)))
    }

    #[cfg(not(windows))]
    fn spawn_child(script: &str) -> (PlatformChild, Box<dyn std::any::Any>) {
        static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
        let runtime = RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().unwrap());
        let _entered = runtime.enter();
        let script = script.replace("ping -n 60 127.0.0.1 >NUL", "sleep 60");
        let child = tokio::process::Command::new("sh")
            .args(["-c", &script])
            .spawn()
            .unwrap();
        (child, Box::new(()))
    }

    #[test]
    #[cfg(all(unix, target_pointer_width = "64"))]
    fn spawn_worker_rifiutato_non_perde_ownership_del_figlio() {
        let supervisor = ReaperSupervisor::new_for_test();
        // 2^60 byte eccedono lo spazio indirizzabile userspace: pthread
        // rifiuta questo stack, senza allocare RAM fisica o limiti host.
        // Nessuna modifica rlimit/config globale, server/file/network.
        let error = supervisor
            .handoff_with_builder(
                short_lived_child(),
                std::thread::Builder::new().stack_size(1usize << 60),
            )
            .unwrap_err();
        assert_ne!(error.to_string(), "", "errore spawn nativo reale");
        let next = supervisor.handoff(short_lived_child()).unwrap();
        let reaped = supervisor.wait_stopped(next, TEST_TIMEOUT).unwrap();
        assert_eq!(
            reaped, 2,
            "anche il figlio del worker non nato resta del supervisore"
        );
    }

    #[test]
    #[cfg(all(unix, target_pointer_width = "64"))]
    fn piu_spawn_falliti_conservano_tutti_i_figli_per_prossimo_handoff() {
        let supervisor = ReaperSupervisor::new_for_test();
        for _ in 0..2 {
            assert!(
                supervisor
                    .handoff_with_builder(
                        short_lived_child(),
                        std::thread::Builder::new().stack_size(1usize << 60)
                    )
                    .is_err()
            );
        }
        let generation = supervisor.handoff(short_lived_child()).unwrap();
        assert_eq!(
            supervisor.wait_stopped(generation, TEST_TIMEOUT).unwrap(),
            3
        );
    }

    #[test]
    fn errore_wait_non_diventa_raccolta_e_conserva_child() {
        let mut children = vec![short_lived_child()];
        let (collected, mut failures) = super::poll_reaper_children(&mut children, |_| {
            Err(std::io::Error::other("fault injection wait: non esito OS"))
        });
        // Cleanup dei child realmente posseduti PRIMA degli assert: il fault
        // è sintetico e non deve lasciare un subprocess della fixture vivo.
        let failed_count = failures.len();
        let supervisor = ReaperSupervisor::new_for_test();
        for (child, _) in failures.drain(..) {
            let generation = supervisor.handoff(child).unwrap();
            supervisor.wait_stopped(generation, TEST_TIMEOUT).unwrap();
        }
        assert_eq!(collected, 0, "Err non è successful native wait");
        assert_eq!(failed_count, 1, "handle con errore rimane posseduto");
        assert!(children.is_empty());
    }

    #[test]
    fn ricevuta_phase_con_wait_fallito_non_conferma_raccolta() {
        // Solo protocol/cache test: non crea worker né prova native exit.
        let supervisor = ReaperSupervisor::new_for_test();
        supervisor.shared.lifecycle.report_stopped(1, 0, 1);
        let error = supervisor.wait_stopped(1, TEST_TIMEOUT).unwrap_err();
        assert!(error.to_string().contains("1 wait falliti, 0 raccolti"));
    }

    #[test]
    fn reaper_esce_a_vuoto_e_raccoglie_il_figlio_successivo() {
        let supervisor = ReaperSupervisor::new_for_test();
        let first = supervisor.handoff(short_lived_child()).unwrap();
        assert_eq!(
            supervisor.wait_reported(first, TEST_TIMEOUT).unwrap(),
            (1, 0)
        );
        assert!(
            !supervisor
                .shared
                .lifecycle
                .joined
                .lock()
                .unwrap()
                .contains(&first)
        );
        let second = supervisor.handoff(short_lived_child()).unwrap();
        assert!(second > first);
        assert!(
            supervisor
                .shared
                .lifecycle
                .joined
                .lock()
                .unwrap()
                .contains(&first)
        );
        assert_eq!(supervisor.wait_stopped(first, TEST_TIMEOUT).unwrap(), 1);
        supervisor.wait_stopped(second, TEST_TIMEOUT).unwrap();
        assert!(
            super::lock(&supervisor.shared.state).worker.is_none(),
            "wait_stopped deve consumare il JoinHandle della generazione finale"
        );
        assert!(
            supervisor
                .shared
                .lifecycle
                .joined
                .lock()
                .unwrap()
                .contains(&second)
        );
    }

    #[test]
    fn reaper_uccide_e_raccoglie_un_figlio_ancora_vivo() {
        let supervisor = ReaperSupervisor::new_for_test();
        let (child, _vivi) = long_lived_child();
        let generation = supervisor.handoff(child).unwrap();
        let raccolti = supervisor.wait_stopped(generation, TEST_TIMEOUT).unwrap();
        assert_eq!(raccolti, 1, "il figlio vivo va ucciso e raccolto");
    }

    #[test]
    fn drain_unisce_il_worker_finale_e_restutuisce_quanti_ne_ha_uniti() {
        let supervisor = ReaperSupervisor::new_for_test();
        let generation = supervisor.handoff(short_lived_child()).unwrap();
        assert_eq!(supervisor.drain(TEST_TIMEOUT).unwrap(), 1);
        assert!(supervisor.shared.state.lock().unwrap().worker.is_none());
        assert!(
            supervisor
                .shared
                .lifecycle
                .joined
                .lock()
                .unwrap()
                .contains(&generation)
        );
        assert_eq!(
            supervisor.drain(TEST_TIMEOUT).unwrap(),
            0,
            "drain idempotente"
        );
    }

    #[test]
    fn drain_pubblico_unisce_il_reaper_globale() {
        let supervisor = DETACHED_REAPER.get_or_init(ReaperSupervisor::new);
        let generation = supervisor.handoff(short_lived_child()).unwrap();
        assert_eq!(super::drain_process_reaper(TEST_TIMEOUT).unwrap(), 1);
        assert!(
            supervisor
                .shared
                .lifecycle
                .joined
                .lock()
                .unwrap()
                .contains(&generation)
        );
    }

    #[test]
    #[cfg(all(unix, target_pointer_width = "64"))]
    fn drain_riprova_i_child_conservati_dopo_spawn_worker_fallito() {
        let supervisor = ReaperSupervisor::new_for_test();
        assert!(
            supervisor
                .handoff_with_builder(
                    short_lived_child(),
                    std::thread::Builder::new().stack_size(1usize << 60),
                )
                .is_err()
        );
        assert_eq!(
            super::lock(&supervisor.shared.state)
                .retained_handoffs
                .len(),
            1
        );
        assert_eq!(supervisor.drain(TEST_TIMEOUT).unwrap(), 1);
        assert!(
            super::lock(&supervisor.shared.state)
                .retained_handoffs
                .is_empty()
        );
    }

    #[test]
    fn drain_non_dichiara_raccolta_un_wait_fallito() {
        let supervisor = ReaperSupervisor::new_for_test();
        supervisor
            .shared
            .state
            .lock()
            .unwrap()
            .wait_failures
            .push((short_lived_child(), std::io::Error::other("wait incerto")));
        assert!(
            supervisor
                .drain(TEST_TIMEOUT)
                .unwrap_err()
                .to_string()
                .contains("1 unconfirmed child waits")
        );
        let child = super::lock(&supervisor.shared.state)
            .wait_failures
            .pop()
            .unwrap()
            .0;
        let cleanup = ReaperSupervisor::new_for_test();
        let generation = cleanup.handoff(child).unwrap();
        cleanup.wait_stopped(generation, TEST_TIMEOUT).unwrap();
    }

    /// Un passaggio di mano che arriva esattamente tra «lista vuota» e
    /// «prendo il lock per uscire» non deve andare perso: il worker lo
    /// drena sotto il lock prima di azzerare il mittente. Il gancio gira sul
    /// worker nel punto esatto del confine, senza alcuna attesa temporizzata.
    #[test]
    fn reaper_non_perde_il_passaggio_al_confine_lista_vuota() {
        let supervisor = ReaperSupervisor::new_for_test();
        let (tx, rx) = mpsc::channel();
        let dal_gancio = supervisor.clone();
        supervisor.set_before_exit_lock_hook(move || {
            let generation = dal_gancio.handoff(short_lived_child()).unwrap();
            tx.send(generation).unwrap();
        });

        let first = supervisor.handoff(short_lived_child()).unwrap();
        let raccolti = supervisor.wait_stopped(first, TEST_TIMEOUT).unwrap();

        let dal_confine = rx.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_eq!(
            dal_confine, first,
            "il mittente e' ancora valido al confine: il figlio entra nello stesso worker"
        );
        assert_eq!(raccolti, 2, "il figlio arrivato al confine non va perso");
    }
}
