use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// A single in-process Guacamole connection to guacd.
///
/// This is a bidirectional byte stream carrying the Guacamole protocol. It
/// implements [`Read`] and [`Write`] (both for `Connection` and `&Connection`,
/// so read and write halves can be used concurrently via [`try_clone`]).
///
/// Dropping the `Connection` closes the underlying socket, which causes guacd
/// to tear down the connection and reap its forked client process.
///
/// [`try_clone`]: Connection::try_clone
pub struct Connection {
    stream: UnixStream,
}

impl Connection {
    pub(crate) fn new(stream: UnixStream) -> Self {
        Connection { stream }
    }

    /// Sets the read timeout for this connection. `None` blocks indefinitely.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }

    /// Sets the write timeout for this connection. `None` blocks indefinitely.
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_write_timeout(timeout)
    }

    /// Moves the connection into or out of nonblocking mode.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.stream.set_nonblocking(nonblocking)
    }

    /// Clones the connection handle so the read and write halves can be owned
    /// by separate threads. Both handles refer to the same underlying socket.
    pub fn try_clone(&self) -> io::Result<Connection> {
        Ok(Connection {
            stream: self.stream.try_clone()?,
        })
    }

    /// Shuts down the read, write, or both halves of the connection.
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.stream.shutdown(how)
    }
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.stream).read(buf)
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&self.stream).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&self.stream).flush()
    }
}

impl Read for &Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.stream).read(buf)
    }
}

impl Write for &Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&self.stream).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&self.stream).flush()
    }
}

#[cfg(feature = "tokio")]
impl Connection {
    /// Converts this connection into an [`AsyncConnection`] driven by tokio.
    ///
    /// This puts the underlying socket into nonblocking mode and registers it
    /// with the tokio I/O driver, so it must be called from within a tokio
    /// runtime (e.g. inside `#[tokio::main]` or a task spawned onto one).
    pub fn into_tokio(self) -> io::Result<AsyncConnection> {
        self.stream.set_nonblocking(true)?;
        let stream = tokio::net::UnixStream::from_std(self.stream)?;
        Ok(AsyncConnection { stream })
    }
}

/// A single in-process Guacamole connection to guacd, driven by tokio.
///
/// Obtained from a [`Connection`] via [`Connection::into_tokio`]. Implements
/// [`tokio::io::AsyncRead`] and [`tokio::io::AsyncWrite`], so the Guacamole
/// protocol stream can be read from and written to using tokio's I/O
/// utilities (e.g. `AsyncReadExt`/`AsyncWriteExt`).
///
/// Requires the `tokio` feature.
#[cfg(feature = "tokio")]
pub struct AsyncConnection {
    stream: tokio::net::UnixStream,
}

#[cfg(feature = "tokio")]
impl AsyncConnection {
    /// Splits the connection into owned read and write halves that can be
    /// driven concurrently by separate tasks.
    pub fn into_split(
        self,
    ) -> (
        tokio::net::unix::OwnedReadHalf,
        tokio::net::unix::OwnedWriteHalf,
    ) {
        self.stream.into_split()
    }

    /// Shuts down the read, write, or both halves of the connection.
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        // tokio's `UnixStream` only exposes write-shutdown (via `AsyncWrite`),
        // so reach for the same primitive `std::os::unix::net::UnixStream`
        // uses to support `Shutdown::Read`/`Both` as well.
        use std::os::fd::AsRawFd;
        let how = match how {
            Shutdown::Read => libc::SHUT_RD,
            Shutdown::Write => libc::SHUT_WR,
            Shutdown::Both => libc::SHUT_RDWR,
        };
        // SAFETY: `fd` is a valid, open socket owned by `self.stream` for the
        // duration of this call.
        if unsafe { libc::shutdown(self.stream.as_raw_fd(), how) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(feature = "tokio")]
impl tokio::io::AsyncRead for AsyncConnection {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

#[cfg(feature = "tokio")]
impl tokio::io::AsyncWrite for AsyncConnection {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::pin::Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
