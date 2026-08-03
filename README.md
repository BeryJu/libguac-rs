# guac-rs

Rust bindings that embed Apache [guacamole-server](https://github.com/apache/guacamole-server)
(`guacd`) **directly inside your process**, modeled on
[`beryJu/osquery-rs`](https://github.com/beryJu/osquery-rs).

Unlike running the `guacd` daemon, this crate starts guacd's machinery
**without binding or listening on any socket** (guacd's normal TCP port 4822 is
never opened). Connections are created programmatically; each is a bidirectional
byte stream over which you speak the Guacamole protocol — exactly as the
Guacamole web tier does over TCP.

```rust
use guacd::{Guacd, LogLevel};
use std::io::{Read, Write};

let guac = Guacd::start(LogLevel::Info)?;
let mut conn = guac.connect()?;

conn.write_all(b"6.select,3.vnc;")?; // choose the VNC protocol
conn.flush()?;

let mut buf = [0u8; 4096];
let n = conn.read(&mut buf)?;         // server "args" instruction, etc.
# let _ = n;
# Ok::<(), guacd::Error>(())
```

## Crates

| Crate       | Purpose |
|-------------|---------|
| `guacd-sys` | Downloads + builds guacamole-server from source, compiles a thin C shim over guacd's own `connection.c`/`proc.c`, and exposes the FFI surface. |
| `guacd`     | Safe API: [`Guacd::start`], [`Guacd::connect`], [`Connection`] (`Read` + `Write`). |

## How it works

- **Start** (`guac_embed_init`) allocates guacd's process map and sets up
  logging. No socket, no bind, no `accept()` loop.
- **Logging** is routed to Rust's [`log`](https://docs.rs/log) crate under the
  `guacd` target, instead of guacd's usual syslog/stderr. A shim `log.c`
  replaces guacd's own `log.c`, framing each message onto a pipe; a background
  reader thread drains it and re-emits via `log`. The pipe hop is what keeps
  the forked protocol clients from calling into the host logger directly —
  unsafe across `fork()` in a multithreaded process. Install any `log` logger
  (`env_logger`, `tracing`, …) to see output.
- **Connect** (`guac_embed_connect`) creates an `AF_UNIX` socketpair, hands one
  end to guacd's stock `guacd_connection_thread`, and returns the other end.
- guacd reads the `select` instruction, **forks** a child process that
  `dlopen`s `libguac-client-<protocol>.so` and runs the protocol client —
  exactly guacd's native fork-per-connection model. Dropping the `Connection`
  ends the connection thread and reaps the forked process.

> **Note on `fork()`:** guacd forks (without `exec`) per connection. In a
> multithreaded host process only the forking thread survives in the child;
> this mirrors upstream guacd's behavior and is the model chosen for this crate.

## Supported targets

Linux `x86_64` and `aarch64` only. Protocols built: **VNC, RDP, SSH**.
(Telnet, Kubernetes, `guacenc`, and `guaclog` are planned — see the plan file.)

## Building

`guacd-sys/build.rs` downloads a pinned, SHA-256-verified guacamole-server
release tarball (currently **1.6.0**) and builds it with autotools. The
following development libraries must be present at build time:

```
build-essential pkg-config autoconf automake libtool libtool-bin
clang libclang-dev
libcairo2-dev libjpeg62-turbo-dev libpng-dev libwebp-dev
libpango1.0-dev uuid-dev libossp-uuid-dev libssl-dev
libvncserver-dev          # VNC
freerdp2-dev libwinpr2-dev # RDP
libssh2-1-dev             # SSH
```

Environment overrides:

- `GUACAMOLE_SERVER_TARBALL=/path/to/guacamole-server-1.6.0.tar.gz` — use a
  local tarball instead of downloading.

### Using Docker (recommended on non-Linux hosts)

```sh
docker build -t guac-rs .
docker run --rm guac-rs                 # runs `cargo test`
docker run --rm guac-rs cargo build     # or any command
# or, with cached volumes:
docker compose run --rm build
```

## Testing

- `cargo test` runs `handshake_forks_vnc_plugin_without_a_listening_socket`,
  which needs **no external server**: it proves guacd forked the VNC client and
  loaded its plugin by asserting the `args` handshake comes back.
- The full connect test needs a real VNC server:
  ```sh
  GUAC_VNC_HOST=127.0.0.1 GUAC_VNC_PORT=5900 \
    cargo test -p guacd -- --ignored --test-threads=1
  ```

### Verifying no socket is opened

After `Guacd::start()`, the process has **no** listening socket:

```sh
ss -ltnp | grep <pid>   # empty; only per-connection socketpairs exist
```

## License

Apache-2.0 (matching guacamole-server).
