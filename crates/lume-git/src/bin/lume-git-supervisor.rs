#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod linux {
    use std::{
        collections::HashMap,
        env,
        ffi::{OsStr, OsString},
        io,
        os::unix::process::{CommandExt, ExitStatusExt},
        process::{Command, ExitStatus, Stdio},
        sync::atomic::{AtomicBool, AtomicI32, Ordering},
        thread,
        time::Duration,
    };

    const PREFIX: &str = "LUME_INTERNAL_GIT_";
    const MODE: &str = "LUME_INTERNAL_GIT_MODE";
    const PROGRAM: &str = "LUME_INTERNAL_GIT_PROGRAM";
    const ARG_COUNT: &str = "LUME_INTERNAL_GIT_ARG_COUNT";
    const ARG_PREFIX: &str = "LUME_INTERNAL_GIT_ARG_";
    const MAX_ARGS: usize = 16_384;
    const POLL: Duration = Duration::from_millis(5);

    static CANCELLED: AtomicBool = AtomicBool::new(false);
    static CHILD_GROUP: AtomicI32 = AtomicI32::new(0);

    struct Configuration {
        program: OsString,
        args: Vec<OsString>,
    }

    pub(super) fn main() -> i32 {
        match run() {
            Ok(status) => exit_code(status),
            Err(error) => {
                eprintln!("Lume Git supervisor: {error}");
                127
            }
        }
    }

    fn run() -> io::Result<ExitStatus> {
        // SAFETY: PR_SET_CHILD_SUBREAPER accepts an integer flag and does not
        // dereference the pointer argument when it is zero.
        if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        install_cancel_handler()?;
        let configuration = parse_configuration(env::vars_os())?;
        let mut command = Command::new(configuration.program);
        command
            .args(configuration.args)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .process_group(0);
        for (key, _) in env::vars_os().filter(|(key, _)| is_internal_key(key)) {
            command.env_remove(key);
        }
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .env("GIT_MERGE_AUTOEDIT", "no");

        let mut git = command.spawn()?;
        let git_group = i32::try_from(git.id())
            .map_err(|_| io::Error::other("Git PID does not fit in pid_t"))?;
        CHILD_GROUP.store(git_group, Ordering::Release);

        let wait_result = wait_for_git(&git, git_group);
        let status = match wait_result {
            Ok(()) => git.wait(),
            Err(error) => {
                let _ = kill_group(git_group);
                let _ = git.wait();
                let _ = terminate_adopted_children();
                return Err(error);
            }
        }?;
        CHILD_GROUP.store(0, Ordering::Release);
        terminate_adopted_children()?;
        Ok(status)
    }

    fn parse_configuration<I>(environment: I) -> io::Result<Configuration>
    where
        I: IntoIterator<Item = (OsString, OsString)>,
    {
        let values: HashMap<OsString, OsString> = environment.into_iter().collect();
        if required(&values, MODE)?.as_os_str() != "1" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid supervisor mode",
            ));
        }
        let program = required(&values, PROGRAM)?.clone();
        let count = required(&values, ARG_COUNT)?
            .to_str()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|count| *count <= MAX_ARGS)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid Git argument count")
            })?;
        let mut args = Vec::with_capacity(count);
        for index in 0..count {
            args.push(required(&values, &format!("{ARG_PREFIX}{index}"))?.clone());
        }
        Ok(Configuration { program, args })
    }

    fn required<'a>(
        values: &'a HashMap<OsString, OsString>,
        key: &str,
    ) -> io::Result<&'a OsString> {
        values
            .get(OsStr::new(key))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing supervisor data"))
    }

    fn is_internal_key(key: &OsStr) -> bool {
        key.to_string_lossy().starts_with(PREFIX)
    }

    fn wait_for_git(git: &std::process::Child, process_group: i32) -> io::Result<()> {
        loop {
            if CANCELLED.load(Ordering::Acquire) {
                kill_group(process_group)?;
            }
            if leader_finished(git.id())? {
                // Keep the leader unreaped while signalling its process group,
                // so the numeric group ID cannot have been recycled.
                kill_group(process_group)?;
                return Ok(());
            }
            thread::sleep(POLL);
        }
    }

    fn leader_finished(pid: u32) -> io::Result<bool> {
        let pid = libc::pid_t::try_from(pid)
            .map_err(|_| io::Error::other("Git PID does not fit in pid_t"))?;
        // SAFETY: siginfo is a valid zeroed output buffer; pid is the direct
        // child just spawned by this process; WNOWAIT preserves it as a
        // process-group identity until the subsequent kill and wait.
        let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
        // SAFETY: arguments satisfy waitid's P_PID contract; no pointer escapes.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: waitid initialized siginfo on success, including the zero
        // result used when the child has not exited.
        Ok(unsafe { info.si_pid() } != 0)
    }

    fn kill_group(group: i32) -> io::Result<()> {
        // SAFETY: group is the process group created by the Command above.
        if unsafe { libc::kill(-group, libc::SIGKILL) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }

    fn terminate_adopted_children() -> io::Result<()> {
        let children_path = format!("/proc/self/task/{}/children", std::process::id());
        loop {
            let children = std::fs::read_to_string(&children_path)?;
            let children = children
                .split_ascii_whitespace()
                .map(|pid| {
                    pid.parse::<libc::pid_t>().map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "invalid child PID")
                    })
                })
                .collect::<io::Result<Vec<_>>>()?;
            if children.is_empty() {
                return Ok(());
            }
            for pid in children {
                // SAFETY: pid was read from this supervisor's direct child
                // list; no unrelated process is targeted.
                if unsafe { libc::kill(pid, libc::SIGKILL) } != 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(error);
                    }
                }
                loop {
                    // SAFETY: pid is an adopted direct child and status is a
                    // valid writable output buffer.
                    let result = unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
                    if result == pid {
                        break;
                    }
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    if error.raw_os_error() == Some(libc::ECHILD) {
                        break;
                    }
                    return Err(error);
                }
            }
        }
    }

    fn install_cancel_handler() -> io::Result<()> {
        // SAFETY: action is a valid local sigaction; the handler only performs
        // a lock-free atomic store, and the mask is initialized before use.
        unsafe {
            let mut action = std::mem::zeroed::<libc::sigaction>();
            action.sa_sigaction = handle_cancel as *const () as usize;
            action.sa_flags = 0;
            if libc::sigemptyset(&mut action.sa_mask) != 0
                || libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut()) != 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    extern "C" fn handle_cancel(_signal: libc::c_int) {
        CANCELLED.store(true, Ordering::Relaxed);
    }

    fn exit_code(status: ExitStatus) -> i32 {
        if let Some(signal) = status.signal() {
            // Preserve the child's signal result after its descendants have
            // been cleaned up, so the caller observes a signalled process.
            // SAFETY: restoring the default action and raising this signal is
            // the intended way to preserve ExitStatusExt::signal semantics.
            unsafe {
                libc::signal(signal, libc::SIG_DFL);
                libc::raise(signal);
            }
            return 128 + signal;
        }
        status.code().unwrap_or(127)
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    let code = linux::main();
    #[cfg(not(target_os = "linux"))]
    let code = {
        eprintln!("Lume Git supervisor: missing supervisor data");
        127
    };
    std::process::exit(code);
}
