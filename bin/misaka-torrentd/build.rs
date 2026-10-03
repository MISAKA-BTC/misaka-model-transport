//! Sets the daemon's rpath to the pinned libtorrent's directory when the `libtorrent` feature
//! links it (the engine's build script reports it through `links` metadata).

fn main() {
    println!("cargo::rerun-if-env-changed=DEP_MISAKA_LT_SHIM_RPATH");
    if let Ok(dirs) = std::env::var("DEP_MISAKA_LT_SHIM_RPATH") {
        for d in dirs.split(':').filter(|d| !d.is_empty()) {
            println!("cargo::rustc-link-arg-bins=-Wl,-rpath,{d}");
        }
    }
}
