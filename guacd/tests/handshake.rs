//! Integration tests exercising the embedded guacd end-to-end.
//!
//! `handshake` needs no external server: it proves guacd forked a client
//! process, `dlopen`ed `libguac-client-vnc.so`, and began the Guacamole
//! handshake. The full connect test is `#[ignore]`d and needs a real VNC
//! server (run with `cargo test -- --ignored --test-threads=1`).

use std::io::{Read, Write};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use guacd::{Connection, Guacd, LogLevel};
use log::{Level, Log, Metadata, Record};

/// Log records captured from the `guacd` target, proving guacd's C-side log
/// output is routed through the [`log`] crate.
static CAPTURED: Mutex<Vec<(Level, String)>> = Mutex::new(Vec::new());

/// Serializes tests that start a [`Guacd`] instance: only one may be live in
/// the process at a time, but cargo runs tests in this file concurrently by
/// default. Acquire this before `Guacd::start` and hold it until `guac` (and
/// thus the instance) is dropped. A `tokio::sync::Mutex` rather than a `std`
/// one, since the async tests hold the guard across `.await` points; the
/// plain sync test uses [`tokio::sync::Mutex::blocking_lock`] instead.
static GUACD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct CaptureLogger;

impl Log for CaptureLogger {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }
    fn log(&self, record: &Record) {
        if record.target() == "guacd" {
            CAPTURED
                .lock()
                .unwrap()
                .push((record.level(), record.args().to_string()));
        }
    }
    fn flush(&self) {}
}

/// Installs [`CaptureLogger`] as the process logger exactly once. Uses
/// `set_logger` with a `'static` instance so the `log` crate's `std` feature
/// (which `set_boxed_logger` needs) is not required.
static CAPTURE_LOGGER: CaptureLogger = CaptureLogger;

fn install_capture_logger() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        log::set_logger(&CAPTURE_LOGGER).expect("install capture logger");
        log::set_max_level(log::LevelFilter::Trace);
    });
}

/// Encodes a single Guacamole protocol instruction:
/// `LEN.ELEM,LEN.ELEM,...;` where LEN is the element's character count.
fn instruction(elements: &[&str]) -> Vec<u8> {
    let mut out = String::new();
    for (i, e) in elements.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!("{}.{}", e.chars().count(), e));
    }
    out.push(';');
    out.into_bytes()
}

/// Counts TCP sockets in the LISTEN state (0x0A) via /proc/net/tcp{,6}.
fn listening_tcp_sockets() -> usize {
    let mut count = 0;
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(contents) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in contents.lines().skip(1) {
            // columns: sl local_address rem_address st ...
            if line.split_whitespace().nth(3) == Some("0A") {
                count += 1;
            }
        }
    }
    count
}

/// Reads until `needle` appears in the stream or the read times out / EOFs.
fn read_until(conn: &mut Connection, needle: &str) -> String {
    let mut acc = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match conn.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                acc.extend_from_slice(&buf[..n]);
                if String::from_utf8_lossy(&acc).contains(needle) {
                    break;
                }
            }
            Err(e) => panic!("read failed before seeing {needle:?}: {e}"),
        }
    }
    String::from_utf8_lossy(&acc).into_owned()
}

#[test]
fn handshake_forks_vnc_plugin_without_a_listening_socket() {
    install_capture_logger();
    let _guard = GUACD_LOCK.blocking_lock();

    let listeners_before = listening_tcp_sockets();
    let guac = Guacd::start(LogLevel::Debug).expect("start guacd");

    // Starting guacd must not open any listening TCP socket (unlike the real
    // guacd daemon, which binds port 4822).
    assert_eq!(
        listening_tcp_sockets(),
        listeners_before,
        "Guacd::start opened a listening TCP socket"
    );

    let mut conn = guac.connect().expect("open connection");
    conn.set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();

    // Choose the VNC protocol. This causes guacd to fork a child, load
    // libguac-client-vnc.so, and reply with the "args" instruction listing the
    // parameters the plugin accepts -- all before any VNC server is contacted.
    conn.write_all(&instruction(&["select", "vnc"])).unwrap();
    conn.flush().unwrap();

    let resp = read_until(&mut conn, "args");
    assert!(
        resp.contains("args"),
        "expected an \"args\" handshake instruction, got: {resp:?}"
    );

    // guacd logs connection lifecycle events (e.g. selecting the protocol and
    // forking the client). Those come from C, are framed onto a pipe, and are
    // re-emitted by the background reader thread through the `log` crate, so
    // give that async hop a moment to catch up.
    let mut captured = Vec::new();
    for _ in 0..100 {
        captured = CAPTURED.lock().unwrap().clone();
        if !captured.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !captured.is_empty(),
        "expected guacd log records to arrive via the log crate, got none"
    );
}

/// Full connect against a real VNC server. Set GUAC_VNC_HOST / GUAC_VNC_PORT
/// (and optionally GUAC_VNC_PASSWORD), then:
///   cargo test -p guacd -- --ignored --test-threads=1
#[test]
#[ignore = "requires a reachable VNC server via GUAC_VNC_HOST/GUAC_VNC_PORT"]
fn full_vnc_connect_streams_display_updates() {
    let host = std::env::var("GUAC_VNC_HOST").expect("GUAC_VNC_HOST");
    let port = std::env::var("GUAC_VNC_PORT").unwrap_or_else(|_| "5900".into());
    let password = std::env::var("GUAC_VNC_PASSWORD").unwrap_or_default();

    let guac = Guacd::start(LogLevel::Debug).expect("start guacd");
    let mut conn = guac.connect().expect("open connection");
    conn.set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();

    // select -> read args
    conn.write_all(&instruction(&["select", "vnc"])).unwrap();
    conn.flush().unwrap();
    let args = read_until(&mut conn, "args");
    assert!(args.contains("args"));

    // Minimal client half of the handshake.
    conn.write_all(&instruction(&["size", "1024", "768", "96"]))
        .unwrap();
    conn.write_all(&instruction(&["audio"])).unwrap();
    conn.write_all(&instruction(&["video"])).unwrap();
    conn.write_all(&instruction(&["image"])).unwrap();

    // The order of connect values must match the received "args" list. For a
    // default libguac-client-vnc build that is: hostname, port, ... password.
    // We send hostname + port and leave the rest blank, appending the password
    // as the final value. This is a smoke test, not a full arg mapper.
    let mut connect = vec!["connect", host.as_str(), port.as_str()];
    // pad remaining args as empty except the last (password)
    let arg_count = args.matches('.').count(); // rough upper bound
    let pad = arg_count.saturating_sub(1).saturating_sub(connect.len());
    connect.resize(connect.len() + pad, "");
    connect.push(password.as_str());
    conn.write_all(&instruction(&connect)).unwrap();
    conn.flush().unwrap();

    // Expect the display to start streaming (sync / img / png / blob opcodes).
    let stream = read_until(&mut conn, "sync");
    assert!(
        stream.contains("sync") || stream.contains("img") || stream.contains("blob"),
        "expected display updates, got: {stream:?}"
    );
}

/// Same handshake as [`handshake_forks_vnc_plugin_without_a_listening_socket`],
/// but driven over the tokio-backed [`guacd::AsyncConnection`] instead of the
/// blocking [`Connection`], proving `Connection::into_tokio` produces a
/// working `AsyncRead`/`AsyncWrite` stream.
#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_handshake_forks_vnc_plugin() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::{Duration, timeout};

    let _guard = GUACD_LOCK.lock().await;
    let guac = Guacd::start(LogLevel::Debug).expect("start guacd");
    let conn = guac.connect().expect("open connection");
    let mut conn = conn.into_tokio().expect("convert to tokio connection");

    conn.write_all(&instruction(&["select", "vnc"]))
        .await
        .unwrap();
    conn.flush().await.unwrap();

    let resp = timeout(Duration::from_secs(20), async {
        let mut acc = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = conn.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            acc.extend_from_slice(&buf[..n]);
            if String::from_utf8_lossy(&acc).contains("args") {
                break;
            }
        }
        String::from_utf8_lossy(&acc).into_owned()
    })
    .await
    .expect("timed out waiting for args instruction");

    assert!(
        resp.contains("args"),
        "expected an \"args\" handshake instruction, got: {resp:?}"
    );
}

/// [`guacd::AsyncConnection::into_split`] should hand back independent
/// read/write halves that can be driven from separate tasks, mirroring the
/// blocking [`Connection::try_clone`] use case.
#[cfg(feature = "tokio")]
#[tokio::test]
async fn async_connection_can_be_split_across_tasks() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::Duration;

    let _guard = GUACD_LOCK.lock().await;
    let guac = Guacd::start(LogLevel::Debug).expect("start guacd");
    let conn = guac.connect().expect("open connection");
    let conn = conn.into_tokio().expect("convert to tokio connection");
    let (mut read_half, mut write_half) = conn.into_split();

    let writer = tokio::spawn(async move {
        write_half
            .write_all(&instruction(&["select", "vnc"]))
            .await
            .unwrap();
        write_half.flush().await.unwrap();
    });

    let reader = tokio::spawn(async move {
        let mut acc = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(20), read_half.read(&mut buf))
                .await
                .expect("timed out waiting for args instruction")
                .unwrap();
            if n == 0 {
                break;
            }
            acc.extend_from_slice(&buf[..n]);
            if String::from_utf8_lossy(&acc).contains("args") {
                break;
            }
        }
        String::from_utf8_lossy(&acc).into_owned()
    });

    writer.await.unwrap();
    let resp = reader.await.unwrap();
    assert!(
        resp.contains("args"),
        "expected an \"args\" handshake instruction, got: {resp:?}"
    );
}
