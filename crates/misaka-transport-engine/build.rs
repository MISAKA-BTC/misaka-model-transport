//! Builds `shim/libtorrent` only with the `libtorrent` feature (RFC-0001 §1.3): the default build
//! needs no C++ toolchain.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    #[cfg(feature = "libtorrent")]
    libtorrent();
}

#[cfg(feature = "libtorrent")]
fn libtorrent() {
    let shim = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../shim/libtorrent");
    println!("cargo::rerun-if-changed={}", shim.display());
    println!("cargo::rerun-if-env-changed=MISAKA_LT_ALLOW_UNPINNED");
    // third_party.toml pins the version; MISAKA_LT_ALLOW_UNPINNED=1 accepts any 2.0.x for a
    // development build.
    let pinned = "2.0.15";
    let mut probe = pkg_config::Config::new();
    // MISAKA_LT_STATIC=1 links libtorrent and its OpenSSL statically, for a self-contained
    // desktop binary (the app bundles the daemon; Homebrew or distro paths must not leak into it).
    println!("cargo::rerun-if-env-changed=MISAKA_LT_STATIC");
    if std::env::var_os("MISAKA_LT_STATIC").is_some() {
        probe.statik(true);
    }
    if std::env::var_os("MISAKA_LT_ALLOW_UNPINNED").is_some() {
        probe.range_version("2.0.0".."2.1.0");
    } else {
        probe.exactly_version(pinned);
    }
    let lt = probe.probe("libtorrent-rasterbar").unwrap_or_else(|e| {
        panic!(
            "the `libtorrent` feature needs libtorrent-rasterbar {pinned} (third_party.toml) visible to \
             pkg-config; set PKG_CONFIG_PATH. {e}"
        )
    });
    // A pinned libtorrent usually lives outside the loader's default path: hand its directory to
    // the binaries that link it, which set their rpath (bin/misaka-torrentd/build.rs).
    let dirs: Vec<String> = lt.link_paths.iter().map(|p| p.display().to_string()).collect();
    println!("cargo::metadata=rpath={}", dirs.join(":"));
    let mut build = cc::Build::new();
    build.cpp(true).std("c++17").file(shim.join("shim.cpp")).include(&shim).warnings(true);
    for p in &lt.include_paths {
        build.include(p);
    }
    for (k, v) in &lt.defines {
        build.define(k, v.as_deref());
    }
    build.compile("misaka_lt_shim");
}
