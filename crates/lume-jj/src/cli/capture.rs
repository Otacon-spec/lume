//! Lettori attivi posseduti: stop esplicito, source non bloccante, un solo join.
use super::{MAX_CAPTURE, child_owner::record_reader_drop};
use std::{
    io::{self, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
const READ_WAIT: Duration = Duration::from_millis(5);

pub(super) struct Capture {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<io::Result<Vec<u8>>>>,
}
impl Capture {
    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = &self.handle {
            handle.thread().unpark();
        }
    }
    fn join(&mut self) -> io::Result<Vec<u8>> {
        self.stop();
        let Some(handle) = self.handle.take() else {
            return Ok(Vec::new());
        };
        handle
            .join()
            .map_err(|_| io::Error::other("lettore pipe JJ terminato con panic"))?
    }
    pub(super) fn finish(mut self, wait: Duration) -> io::Result<String> {
        let start = Instant::now();
        while self
            .handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
            && start.elapsed() < wait
        {
            std::thread::sleep(READ_WAIT);
        }
        // Deadline di drain non dimostra EOF; fermiamo la source prima del
        // join, anche se un discendente tiene ancora aperto il writer.
        let bytes = self.join()?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
    pub(super) fn abort(mut self) -> io::Result<()> {
        self.join().map(|_| ())
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        if self.handle.is_some()
            && let Err(error) = self.join()
        {
            record_reader_drop(error);
        }
    }
}

pub(super) trait Pipe: Read + Send + 'static {
    fn prepare(&self) -> io::Result<()>;
    fn available(&mut self, bytes: &mut [u8]) -> io::Result<usize>;
}
#[cfg(unix)]
impl<R: Read + Send + 'static + std::os::fd::AsRawFd> Pipe for R {
    fn prepare(&self) -> io::Result<()> {
        // SAFETY: fd vivo/posseduto dalla source, flag precedenti preservati.
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
        // SAFETY: handle owned e unico consumer; peek non consuma. Nessun
        // altro reader può sottrarre i byte prima di questa read limitata.
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
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::BrokenPipe {
                return Ok(0);
            }
            return Err(error);
        }
        if ready == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let count = bytes.len().min(ready as usize);
        self.read(
            bytes
                .get_mut(..count)
                .ok_or_else(|| io::Error::other("intervallo pipe JJ non valido"))?,
        )
    }
}

pub(super) fn drain<R: Pipe>(mut source: R) -> io::Result<Capture> {
    source.prepare()?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let handle = std::thread::Builder::new()
        .name("lume-jj-pipe".into())
        .spawn(move || {
            let mut kept = Vec::new();
            let mut chunk = [0u8; 4096];
            while !flag.load(Ordering::Acquire) {
                match source.available(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        let take = read.min(MAX_CAPTURE.saturating_sub(kept.len()));
                        let part = chunk
                            .get(..take)
                            .ok_or_else(|| io::Error::other("read pipe JJ oltre il buffer"))?;
                        kept.extend_from_slice(part);
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        // Token unpark conservato anche se stop precede park;
                        // polling solo mentre questa pipe attiva è posseduta.
                        std::thread::park_timeout(READ_WAIT);
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(kept)
        })?;
    Ok(Capture {
        stop,
        handle: Some(handle),
    })
}

/// Consumare entrambi prima di propagare errori: il secondo non è saltato
/// neanche quando il primo join/read fallisce. Catture opzionali solo nel setup.
pub(super) fn finish_all(
    stdout: Option<Capture>,
    stderr: Option<Capture>,
    wait: Duration,
) -> io::Result<(String, String)> {
    let out = stdout.map(|capture| capture.finish(wait)).transpose();
    let err = stderr.map(|capture| capture.finish(wait)).transpose();
    match (out, err) {
        (Ok(out), Ok(err)) => Ok((out.unwrap_or_default(), err.unwrap_or_default())),
        (Err(out), Err(err)) => Err(io::Error::other(format!(
            "stdout JJ: {out}; stderr JJ: {err}"
        ))),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}
pub(super) fn abort_all(stdout: Option<Capture>, stderr: Option<Capture>) -> io::Result<()> {
    let out = stdout.map(Capture::abort).transpose();
    let err = stderr.map(Capture::abort).transpose();
    match (out, err) {
        (Ok(_), Ok(_)) => Ok(()),
        (Err(out), Err(err)) => Err(io::Error::other(format!(
            "stdout JJ: {out}; stderr JJ: {err}"
        ))),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}
