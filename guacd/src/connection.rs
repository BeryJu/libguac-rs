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
