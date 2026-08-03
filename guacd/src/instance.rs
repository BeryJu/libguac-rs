use std::fs::File;
use std::io::{BufReader, Read};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Connection, Error, LogLevel};

/// Guards against more than one live guacd instance per process. guacd's
/// logging state and process map are process-global, so a second instance
/// would clobber the first.
static INSTANCE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// An embedded guacd instance.
///
/// Created with [`Guacd::start`]. Dropping it releases guacd's process map;
/// individual [`Connection`]s are independent and end when their stream is
/// dropped.
pub struct Guacd {
    ctx: *mut guacd_sys::guac_embed_ctx,
}

// The context is only ever touched behind `&self`, and guacd's connection
// threads own their own state. Sharing the handle across threads is safe.
unsafe impl Send for Guacd {}
unsafe impl Sync for Guacd {}

impl Guacd {
    /// Starts guacd's in-process machinery. **No socket is created or bound.**
    ///
    /// Returns [`Error::AlreadyRunning`] if another [`Guacd`] is still alive in
    /// this process.
    pub fn start(log_level: LogLevel) -> Result<Self, Error> {
        if INSTANCE_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Error::AlreadyRunning);
        }

        // SAFETY: FFI call into the shim; returns NULL only on allocation
        // failure.
        let ctx = unsafe { guacd_sys::guac_embed_init(log_level as i32) };
        if ctx.is_null() {
            INSTANCE_ACTIVE.store(false, Ordering::Release);
            return Err(Error::Start);
        }

        // Drain guacd's log pipe into the `log` crate on a background thread.
        // SAFETY: `ctx` is a valid context just returned by init.
        let log_fd = unsafe { guacd_sys::guac_embed_log_fd(ctx) };
        if log_fd >= 0 {
            spawn_log_reader(log_fd);
        }

        Ok(Guacd { ctx })
    }

    /// Opens a new in-process Guacamole connection.
    ///
    /// The returned [`Connection`] is a bidirectional byte stream. Write the
    /// Guacamole handshake (`select`, then the protocol arguments) to it and
    /// read the protocol instruction stream back. guacd forks the protocol
    /// client process lazily, on the first `select` instruction.
    pub fn connect(&self) -> Result<Connection, Error> {
        // SAFETY: `self.ctx` is a valid context for the lifetime of `self`.
        let fd = unsafe { guacd_sys::guac_embed_connect(self.ctx) };
        if fd < 0 {
            return Err(Error::Connect(std::io::Error::last_os_error()));
        }

        // SAFETY: `fd` is a freshly-created, owned socketpair endpoint that no
        // one else holds.
        let stream = unsafe { UnixStream::from_raw_fd(fd) };
        Ok(Connection::new(stream))
    }
}

impl Drop for Guacd {
    fn drop(&mut self) {
        // SAFETY: called exactly once, with the context from `start`. This
        // closes the write end of the log pipe; the reader thread below then
        // sees EOF (once any forked children have also exited) and stops.
        unsafe { guacd_sys::guac_embed_shutdown(self.ctx) };
        INSTANCE_ACTIVE.store(false, Ordering::Release);
    }
}

/// Spawns a detached thread that reads framed log records from guacd's log
/// pipe and re-emits each one through the [`log`] crate under the `guacd`
/// target.
///
/// The thread is detached rather than joined: [`Connection`]s (and thus the
/// forked client processes that keep write ends of the pipe open) are
/// independent of the [`Guacd`] lifetime, so the pipe may not reach EOF until
/// after `Guacd` is dropped. The thread owns `fd` and exits on EOF, closing it.
fn spawn_log_reader(fd: RawFd) {
    // SAFETY: `fd` is the read end of a freshly-created pipe that no one else
    // holds; the thread owns it for its entire lifetime.
    let file = unsafe { File::from_raw_fd(fd) };

    let spawned = std::thread::Builder::new()
        .name("guacd-log".into())
        .spawn(move || {
            let mut reader = BufReader::new(file);
            // Record framing (see shim/log.c): [level:1][len:2 LE][msg:len].
            let mut header = [0u8; 3];
            let mut msg = Vec::new();
            loop {
                if reader.read_exact(&mut header).is_err() {
                    break; // EOF (all writers gone) or a read error.
                }
                let level = guac_level_to_log(header[0]);
                let len = u16::from_le_bytes([header[1], header[2]]) as usize;

                msg.clear();
                msg.resize(len, 0);
                if reader.read_exact(&mut msg).is_err() {
                    break;
                }

                log::log!(target: "guacd", level, "{}", String::from_utf8_lossy(&msg));
            }
        });

    // A failure to spawn the reader is non-fatal: guacd keeps running, we just
    // drop its log output. The closure (and with it the owned `file`) is
    // dropped on failure, so the read fd is closed here.
    if let Err(e) = spawned {
        log::warn!(target: "guacd", "failed to spawn guacd log reader thread: {e}");
    }
}

/// Maps a guacamole `guac_client_log_level` to a [`log::Level`].
fn guac_level_to_log(level: u8) -> log::Level {
    match level {
        3 => log::Level::Error,   // GUAC_LOG_ERROR
        4 => log::Level::Warn,    // GUAC_LOG_WARNING
        5 | 6 => log::Level::Info, // GUAC_LOG_INFO (5 is unused by guacd)
        7 => log::Level::Debug,   // GUAC_LOG_DEBUG
        _ => log::Level::Trace,   // GUAC_LOG_TRACE (8) and anything higher
    }
}
