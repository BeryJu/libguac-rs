//! Build script for `guacd-sys`.
//!
//! Steps:
//!   1. Guard the target (linux x86_64 / aarch64 only).
//!   2. Download a pinned, SHA-256-verified guacamole-server release tarball
//!      (or use a local override) and extract it.
//!   3. `./configure --prefix=<OUT_DIR>/guac-install && make && make install`,
//!      building libguac and the VNC/RDP/SSH protocol plugins from source.
//!   4. Compile guacd's connection/proc sources + our C shim into a static lib.
//!   5. Emit link directives (libguac + rpath) and generate bindgen bindings.

use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Pinned guacamole-server release.
const VERSION: &str = "1.6.0";
/// SHA-256 of guacamole-server-<VERSION>.tar.gz (from the Apache dist mirror).
const SHA256: &str = "8bc45675da96d7b6f39728160181e3d4ff3c08f460f6d26de5805b642bf13f2b";

/// Protocol plugins we expect the build to produce.
const PROTOCOLS: &[&str] = &["vnc", "rdp", "ssh"];

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    if target_os != "linux" || !(target_arch == "x86_64" || target_arch == "aarch64") {
        panic!(
            "guacd-sys only supports linux on x86_64 or aarch64 (got {target_os}/{target_arch})"
        );
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let srcroot = out_dir.join(format!("guacamole-server-{VERSION}"));
    let install = out_dir.join("guac-install");

    // --- 1 & 2. fetch + extract -------------------------------------------
    if !srcroot.join("configure").exists() {
        let tarball = obtain_tarball(&out_dir);
        verify_sha256(&tarball);
        extract(&tarball, &out_dir);
    }

    // --- 3. configure + build + install -----------------------------------
    let libguac = install.join("lib").join("libguac.so");
    if !libguac.exists() {
        configure_and_build(&srcroot, &install);
    }

    // Fail loudly if a protocol dev library was missing at configure time.
    // Plugins install directly into <prefix>/lib and are dlopen'd by their bare
    // soname (GUAC_PROTOCOL_LIBRARY_PREFIX = "libguac-client-"), resolved via
    // the rpath baked into libguac.so and the final binary.
    for proto in PROTOCOLS {
        let plugin = install
            .join("lib")
            .join(format!("libguac-client-{proto}.so"));
        if !plugin.exists() {
            panic!(
                "expected plugin {} was not built.\n\
                 The development libraries for the {proto} protocol are probably missing.\n\
                 See the README for the required system packages.",
                plugin.display()
            );
        }
    }

    // --- 4. compile guacd sources + shim ----------------------------------
    let mut build = cc::Build::new();
    build
        .include(&srcroot) // config.h
        .include(srcroot.join("src").join("common")) // "common/list.h"
        .include(srcroot.join("src").join("guacd")) // "log.h", "proc.h", ...
        .include(install.join("include")) // guacamole/*.h
        .include("shim");

    // guacd's own log.c (syslog + stderr) is deliberately excluded; shim/log.c
    // provides the same symbols but delivers log output to the host over a pipe.
    for file in ["connection.c", "proc.c", "proc-map.c", "move-fd.c"] {
        build.file(srcroot.join("src").join("guacd").join(file));
    }
    // guacd's proc-map depends on guac_common_list, which is part of libguac's
    // internal (non-exported) common library, so we compile it in directly.
    build.file(srcroot.join("src").join("common").join("list.c"));
    build.file("shim/shim.c");
    build.file("shim/log.c");

    build
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-deprecated-declarations");
    build.compile("guacembed");

    // --- 5. link directives -----------------------------------------------
    println!(
        "cargo:rustc-link-search=native={}",
        install.join("lib").display()
    );
    println!("cargo:rustc-link-lib=dylib=guac");
    println!("cargo:rustc-link-lib=dylib=pthread");
    println!("cargo:rustc-link-lib=dylib=dl");
    // Resolve libguac (and thus its transitive deps) at runtime without
    // requiring LD_LIBRARY_PATH.
    println!(
        "cargo:rustc-link-arg=-Wl,-rpath,{}",
        install.join("lib").display()
    );

    // --- bindgen -----------------------------------------------------------
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .allowlist_function("guac_embed_.*")
        .allowlist_type("guac_embed_ctx")
        // Don't copy the C doc comments into the bindings: their indented
        // fragments (e.g. the log record layout) get parsed as Rust doctests.
        .generate_comments(false)
        .generate()
        .expect("unable to generate bindings for shim.h");
    bindings
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("unable to write bindings.rs");

    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=shim/shim.h");
    println!("cargo:rerun-if-changed=shim/shim.c");
    println!("cargo:rerun-if-changed=shim/log.c");
    println!("cargo:rerun-if-env-changed=GUACAMOLE_SERVER_TARBALL");
}

/// Returns the path to the source tarball, either from the
/// `GUACAMOLE_SERVER_TARBALL` override or by downloading the pinned release.
fn obtain_tarball(out_dir: &Path) -> PathBuf {
    let dest = out_dir.join(format!("guacamole-server-{VERSION}.tar.gz"));

    if let Ok(local) = env::var("GUACAMOLE_SERVER_TARBALL") {
        println!("cargo:warning=using local tarball {local}");
        fs::copy(&local, &dest).expect("failed to copy GUACAMOLE_SERVER_TARBALL");
        return dest;
    }

    if dest.exists() {
        return dest;
    }

    let url = format!(
        "https://archive.apache.org/dist/guacamole/{VERSION}/source/guacamole-server-{VERSION}.tar.gz"
    );
    println!("cargo:warning=downloading {url}");

    let resp = ureq::get(&url)
        .call()
        .unwrap_or_else(|e| panic!("failed to download {url}: {e}"));
    let mut bytes = Vec::new();
    resp.into_reader()
        .read_to_end(&mut bytes)
        .expect("failed to read tarball body");
    fs::write(&dest, &bytes).expect("failed to write tarball");
    dest
}

fn verify_sha256(path: &Path) {
    use sha2::{Digest, Sha256};
    let bytes = fs::read(path).expect("failed to read tarball for hashing");
    let digest = Sha256::digest(&bytes);
    let got = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
    if got != SHA256 {
        panic!(
            "SHA-256 mismatch for {}\n  expected {SHA256}\n  got      {got}",
            path.display()
        );
    }
}

fn extract(tarball: &Path, out_dir: &Path) {
    let file = fs::File::open(tarball).expect("failed to open tarball");
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    archive
        .unpack(out_dir)
        .expect("failed to extract guacamole-server tarball");
}

fn configure_and_build(srcroot: &Path, install: &Path) {
    // Protocol support is auto-detected from installed dev libraries; the
    // plugin-existence check in main() turns a missing lib into a clear error.
    run(
        Command::new("./configure")
            .current_dir(srcroot)
            .arg(format!("--prefix={}", install.display()))
            .arg("--disable-static")
            .arg("--disable-dependency-tracking"),
        "configure",
    );

    let jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    run(
        Command::new("make")
            .current_dir(srcroot)
            .arg(format!("-j{jobs}")),
        "make",
    );
    run(
        Command::new("make").current_dir(srcroot).arg("install"),
        "make install",
    );
}

fn run(cmd: &mut Command, what: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn `{what}`: {e}"));
    if !status.success() {
        panic!("`{what}` failed with status {status}");
    }
}
