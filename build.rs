//! Builds libre and baresip (vendor/, see tools/fetch-deps.sh) as static libraries, with
//! only the modules a voice softphone needs, and links them in. What stays dynamic is
//! what every Hyprland desktop already has: PipeWire, ALSA, Opus and OpenSSL.

use std::path::{Path, PathBuf};

/// baresip modules compiled in. Audio goes through PipeWire, with ALSA as the fallback.
const MODULES: &str = "g711;opus;pipewire;alsa;stun;turn;ice;auresamp;auconv;srtp;aufile";

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let re_src = root.join("vendor/re");
    let bs_src = root.join("vendor/baresip");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor/re/.dialo-tag");
    println!("cargo:rerun-if-changed=vendor/baresip/.dialo-tag");
    if !re_src.join("CMakeLists.txt").exists() || !bs_src.join("CMakeLists.txt").exists() {
        panic!("\n\nlibre and baresip are missing: run tools/fetch-deps.sh first.\n\n");
    }

    // libre, installed into OUT_DIR so baresip's find_package can see it.
    let re_prefix = cmake::Config::new(&re_src)
        .profile("Release")
        .define("LIBRE_BUILD_SHARED", "OFF")
        .define("LIBRE_BUILD_STATIC", "ON")
        .define("USE_BFCP", "OFF")
        .define("USE_RTMP", "OFF")
        .define("USE_PCP", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .build();

    // baresip: only its library target, with the modules folded in.
    let bs_out = cmake::Config::new(&bs_src)
        .profile("Release")
        .define("STATIC", "ON")
        .define("MODULES", MODULES)
        .define("CMAKE_PREFIX_PATH", &re_prefix)
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .build_target("baresip")
        .build();

    let bs_lib = find(&bs_out.join("build"), "libbaresip.a").expect("libbaresip.a was not built");
    let re_lib = find(&re_prefix, "libre.a").expect("libre.a was not installed");
    println!("cargo:rustc-link-search=native={}", bs_lib.parent().unwrap().display());
    println!("cargo:rustc-link-search=native={}", re_lib.parent().unwrap().display());
    println!("cargo:rustc-link-lib=static=baresip");
    println!("cargo:rustc-link-lib=static=re");

    for lib in ["opus", "libpipewire-0.3", "alsa", "openssl", "zlib"] {
        pkg_config::Config::new()
            .probe(lib)
            .unwrap_or_else(|e| panic!("\n\n{lib} (its -dev package) is needed to build Dialo: {e}\n\n"));
    }
    for lib in ["pthread", "m", "dl", "resolv"] {
        println!("cargo:rustc-link-lib={lib}");
    }
}

fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == name) {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find(&path, name) {
                return Some(found);
            }
        }
    }
    None
}
