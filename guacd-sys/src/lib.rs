//! Low-level FFI bindings to the embedded guacd shim.
//!
//! The native `guacamole-server` source (libguac + VNC/RDP/SSH plugins) is
//! downloaded and built by `build.rs`; guacd's own `connection.c`/`proc.c` are
//! compiled together with a thin C shim (`shim/shim.c`) that exposes the tiny
//! surface bound below. See the `guacd` crate for the safe API.
//!
//! Only the `guac_embed_*` functions and the opaque `guac_embed_ctx` type are
//! generated here.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
