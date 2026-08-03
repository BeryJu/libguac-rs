//! Integration tests exercising the embedded guacd end-to-end.
//!
//! `handshake` needs no external server: it proves guacd forked a client
//! process, `dlopen`ed `libguac-client-vnc.so`, and began the Guacamole
//! handshake. The full connect test is `#[ignore]`d and needs a real VNC
//! server (run with `cargo test -- --ignored --test-threads=1`).

use std::io::{Read, Write};
use std::time::Duration;

use guacd::{Connection, Guacd, LogLevel};

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
    let listeners_before = listening_tcp_sockets();
    let guac = Guacd::start(LogLevel::Info).expect("start guacd");

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
