fn main() {
    println!("cargo:rerun-if-changed=src/pw_bridge.c");
    println!("cargo:rerun-if-changed=src/pw_bridge.h");

    let pw_inc = pkg_config::Config::new()
        .cargo_metadata(false)
        .atleast_version("0.3")
        .probe("libpipewire-0.3")
        .expect("libpipewire-0.3 development headers not found via pkg-config");

    let mut build = cc::Build::new();
    build
        .file("src/pw_bridge.c")
        .flag_if_supported("-std=gnu11")
        .flag_if_supported("-Wno-unused-parameter")
        .opt_level(2);

    for path in &pw_inc.include_paths {
        build.include(path);
    }

    /* Emit -lpw_bridge BEFORE -lpipewire-0.3 so --as-needed linkers resolve symbols cleanly */
    build.compile("pw_bridge");

    for lib in [
        "libpipewire-0.3",
        "egl",
        "glesv2",
        "wayland-client",
        "wayland-egl",
    ] {
        pkg_config::Config::new()
            .probe(lib)
            .unwrap_or_else(|e| panic!("failed to probe {lib} via pkg-config: {e}"));
    }
}
