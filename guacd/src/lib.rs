//! Embed Apache [guacamole-server](https://github.com/apache/guacamole-server)
//! (`guacd`) directly inside a Rust process.
//!
//! Unlike running the `guacd` daemon, [`Guacd::start`] brings up guacd's
//! machinery **without binding or listening on any socket**. Connections are
//! created on demand with [`Guacd::connect`], each returning a [`Connection`]
//! byte stream. The caller speaks the Guacamole protocol over that stream —
//! exactly as the Guacamole web tier does over a TCP connection to guacd:
//!
//! ```no_run
//! use guacd::{Guacd, LogLevel};
//! use std::io::{Read, Write};
//!
//! let guac = Guacd::start(LogLevel::Info)?;
//! let mut conn = guac.connect()?;
//!
//! // Choose the VNC protocol, then perform the Guacamole handshake.
//! conn.write_all(b"6.select,3.vnc;")?;
//! conn.flush()?;
//!
//! let mut buf = [0u8; 4096];
//! let n = conn.read(&mut buf)?; // e.g. the server "args" instruction
//! # let _ = n;
//! # Ok::<(), guacd::Error>(())
//! ```
//!
//! Internally each connection uses guacd's native fork-per-connection model:
//! guacd forks a child that `dlopen`s `libguac-client-<protocol>.so` and runs
//! the protocol client, communicating over an `AF_UNIX` socketpair whose other
//! end is the [`Connection`].

mod connection;
mod instance;

pub use connection::Connection;
pub use instance::Guacd;

/// Maximum severity of log messages guacd should emit. Values match
/// guacamole's `guac_client_log_level` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum LogLevel {
    /// Fatal / error conditions only.
    Error = 3,
    /// Warnings and errors.
    Warning = 4,
    /// Informational messages (guacd's default).
    Info = 6,
    /// Debug-level detail.
    Debug = 7,
    /// Maximum verbosity.
    Trace = 8,
}

/// Errors produced by this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A guacd instance already exists in this process. guacd's logging and
    /// process map are process-global, so only one [`Guacd`] may be live at a
    /// time.
    #[error("a guacd instance is already running in this process")]
    AlreadyRunning,

    /// `guac_embed_init` failed (allocation failure).
    #[error("failed to start guacd")]
    Start,

    /// Creating a new connection failed.
    #[error("failed to create guacd connection: {0}")]
    Connect(#[source] std::io::Error),

    /// An I/O error occurred while reading from or writing to a [`Connection`].
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}
