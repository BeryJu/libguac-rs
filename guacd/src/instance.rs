use std::os::fd::FromRawFd;
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
        // SAFETY: called exactly once, with the context from `start`.
        unsafe { guacd_sys::guac_embed_shutdown(self.ctx) };
        INSTANCE_ACTIVE.store(false, Ordering::Release);
    }
}
