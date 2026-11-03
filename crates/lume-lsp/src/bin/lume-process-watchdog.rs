#[cfg(any(target_os = "linux", target_os = "macos"))]
// `libc`: segnali, `kqueue`, `prctl`. Tutto l'`unsafe` del file sta qui dentro.
#[allow(unsafe_code)]
mod unix_helper {
    use std::{
        env,
        io::{self, Write},
        os::unix::{
            net::UnixStream,
            process::{CommandExt, ExitStatusExt},
        },
        process::{Child, Command, ExitStatus, Stdio},
    };

    #[cfg(target_os = "macos")]
    use std::{
        os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        ptr,
    };

    use lume_lsp::process_watchdog::{Readiness, WatchdogCommand, is_internal_environment_key};

    const WATCHDOG_FAILURE: i32 = 127;

    pub(super) fn main() -> i32 {
        let configuration = match WatchdogCommand::from_environment(env::vars_os()) {
            Ok(configuration) => configuration,
            Err(error) => {
                eprintln!("invalid Lume process-watchdog configuration: {error}");
                return WATCHDOG_FAILURE;
            }
        };
        match run(&configuration) {
            Ok(status) => exit_code(status),
            Err(error) => {
                let _ = notify(&configuration, Readiness::Error(error.to_string()));
                eprintln!("Lume process watchdog failed: {error}");
                WATCHDOG_FAILURE
            }
        }
    }

    fn run(configuration: &WatchdogCommand) -> io::Result<ExitStatus> {
        // SAFETY: `getpid` non ha precondizioni e non tocca la memoria del processo.
        let process_group = unsafe { libc::getpid() };
        // SAFETY: `getpgrp` non ha precondizioni e non tocca la memoria del processo.
        if process_group <= 1 || unsafe { libc::getpgrp() } != process_group {
            return Err(io::Error::other(
                "process watchdog is not its process-group leader",
            ));
        }
        let parent = i32::try_from(configuration.parent_pid)
            .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
        let monitor = ParentMonitor::new(parent)?;

        let mut command = Command::new(&configuration.program);
        command
            .args(&configuration.args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .process_group(process_group);
        if let Some(cwd) = configuration.cwd.as_ref() {
            command.current_dir(cwd);
        }
        for (key, _) in env::vars_os().filter(|(key, _)| is_internal_environment_key(key)) {
            command.env_remove(key);
        }
        command.envs(configuration.env.iter().cloned());
        let mut server = command.spawn()?;
        let server_pid = server.id();
        if let Err(error) = monitor.register_server(server_pid) {
            if let Some(status) = server.try_wait()? {
                notify(configuration, Readiness::Ready { server_pid })?;
                return Ok(status);
            }
            terminate_group(process_group);
            return Err(error);
        }
        if let Err(error) = notify(configuration, Readiness::Ready { server_pid }) {
            terminate_group(process_group);
            return Err(error);
        }

        monitor.wait(&mut server, process_group)
    }

    #[cfg(target_os = "linux")]
    struct ParentMonitor;

    #[cfg(target_os = "linux")]
    impl ParentMonitor {
        fn new(parent: i32) -> io::Result<Self> {
            // SAFETY: `sigset_t` e `sigaction` sono locali azzerate (valori validi per queste strutture C) e vivono fino al ritorno di ogni chiamata; `getppid` e `prctl` non hanno precondizioni sulla memoria.
            unsafe {
                let mut empty = std::mem::zeroed::<libc::sigset_t>();
                if libc::sigemptyset(&mut empty) != 0
                    || libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut()) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                let mut action = std::mem::zeroed::<libc::sigaction>();
                action.sa_sigaction = parent_exited as *const () as usize;
                action.sa_flags = libc::SA_RESTART;
                if libc::sigemptyset(&mut action.sa_mask) != 0
                    || libc::sigaction(libc::SIGUSR1, &action, std::ptr::null_mut()) != 0
                    || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGUSR1) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "IDE parent exited before watchdog startup",
                    ));
                }
            }
            Ok(Self)
        }

        fn register_server(&self, _server_pid: u32) -> io::Result<()> {
            Ok(())
        }

        fn wait(&self, server: &mut Child, _process_group: i32) -> io::Result<ExitStatus> {
            server.wait()
        }
    }

    #[cfg(target_os = "linux")]
    extern "C" fn parent_exited(_signal: libc::c_int) {
        // SAFETY: `getpgrp` è async-signal-safe e non ha precondizioni.
        let process_group = unsafe { libc::getpgrp() };
        if process_group > 1 {
            // SAFETY: `kill` è async-signal-safe e riceve solo interi: `-process_group` indica il gruppo, non un puntatore.
            unsafe {
                libc::kill(-process_group, libc::SIGKILL);
            }
        }
        // SAFETY: `_exit` è async-signal-safe e non ritorna: è l'uscita corretta da un handler di segnale.
        unsafe { libc::_exit(WATCHDOG_FAILURE) }
    }

    #[cfg(target_os = "macos")]
    struct ParentMonitor {
        queue: OwnedFd,
        parent_pid: u32,
    }

    #[cfg(target_os = "macos")]
    impl ParentMonitor {
        fn new(parent: i32) -> io::Result<Self> {
            // SAFETY: `kqueue` non ha precondizioni; l'esito viene controllato subito dopo.
            let descriptor = unsafe { libc::kqueue() };
            if descriptor == -1 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `descriptor` è un descrittore appena creato da `kqueue` e diverso da -1 (controllato sopra), posseduto solo da qui: `OwnedFd` ne prende la proprietà.
            let queue = unsafe { OwnedFd::from_raw_fd(descriptor) };
            register_process(queue.as_raw_fd(), parent as u32)?;
            // SAFETY: `getppid` non ha precondizioni e non tocca la memoria del processo.
            if unsafe { libc::getppid() } != parent {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "IDE parent exited before watchdog startup",
                ));
            }
            Ok(Self {
                queue,
                parent_pid: parent as u32,
            })
        }

        fn register_server(&self, server_pid: u32) -> io::Result<()> {
            register_process(self.queue.as_raw_fd(), server_pid)
        }

        fn wait(&self, server: &mut Child, process_group: i32) -> io::Result<ExitStatus> {
            wait_for_parent_or_server(
                self.queue.as_raw_fd(),
                self.parent_pid,
                server,
                process_group,
            )
        }
    }

    #[cfg(target_os = "macos")]
    fn register_process(queue: RawFd, process_id: u32) -> io::Result<()> {
        // SAFETY: `kevent` è una struttura C di soli interi e puntatori: l'azzeramento è un valore valido.
        let mut event = unsafe { std::mem::zeroed::<libc::kevent>() };
        event.ident = process_id as libc::uintptr_t;
        event.filter = libc::EVFILT_PROC;
        event.flags = libc::EV_ADD | libc::EV_ENABLE | libc::EV_CLEAR;
        event.fflags = libc::NOTE_EXIT;
        // SAFETY: `queue` è un descrittore kqueue vivo; `event` è una locale viva per tutta la chiamata e la lista di uscita è nulla con capacità 0.
        let registered = unsafe { libc::kevent(queue, &event, 1, ptr::null_mut(), 0, ptr::null()) };
        if registered == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn wait_for_parent_or_server(
        queue: RawFd,
        parent_pid: u32,
        server: &mut Child,
        process_group: i32,
    ) -> io::Result<ExitStatus> {
        loop {
            // SAFETY: `kevent` è una struttura C di soli interi e puntatori: l'azzeramento è un valore valido.
            let mut event = unsafe { std::mem::zeroed::<libc::kevent>() };
            // SAFETY: `queue` è un descrittore kqueue vivo; `event` è una locale viva e scrivibile per un solo elemento, che è la capacità passata; nessun elenco di modifiche e nessun timeout.
            let received =
                unsafe { libc::kevent(queue, ptr::null(), 0, &mut event, 1, ptr::null()) };
            if received == -1 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                terminate_group(process_group);
                return Err(error);
            }
            let process_id = event.ident as u32;
            if process_id == parent_pid {
                terminate_group(process_group);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "IDE parent exited",
                ));
            }
            if process_id == server.id() {
                return server.wait();
            }
        }
    }

    fn notify(configuration: &WatchdogCommand, readiness: Readiness) -> io::Result<()> {
        let mut stream = UnixStream::connect(&configuration.readiness_socket)?;
        stream.write_all(&readiness.encode())?;
        stream.shutdown(std::net::Shutdown::Write)
    }

    fn terminate_group(process_group: i32) {
        // SAFETY: `kill` riceve solo interi: `-process_group` indica il gruppo, non un puntatore.
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }

    fn exit_code(status: ExitStatus) -> i32 {
        status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal))
            .unwrap_or(WATCHDOG_FAILURE)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main() {
    std::process::exit(unix_helper::main());
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main() {
    eprintln!("lume-process-watchdog is only used on Linux and macOS");
    std::process::exit(2);
}
