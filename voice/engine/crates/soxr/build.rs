// Compile libsoxr from vendored C sources.

use std::env;

fn main() {
    let mut build = cc::Build::new();

    build.include("src/c");
    build.define("SOXR_LIB", "0");

    build
        .flag_if_supported("-std=gnu89")
        .flag_if_supported("-Wnested-externs")
        .flag_if_supported("-Wmissing-prototypes")
        .flag_if_supported("-Wstrict-prototypes")
        .flag_if_supported("-Wconversion")
        .flag_if_supported("-Wall")
        .flag_if_supported("-Wextra")
        .flag_if_supported("-pedantic")
        .flag_if_supported("-Wundef")
        .flag_if_supported("-Wpointer-arith")
        .flag_if_supported("-Wno-long-long");

    let sources = [
        "src/c/soxr.c",
        "src/c/data-io.c",
        "src/c/dbesi0.c",
        "src/c/filter.c",
        "src/c/cr.c",
        "src/c/cr32.c",
        "src/c/fft4g32.c",
        "src/c/fft4g.c",
        "src/c/fft4g64.c",
        "src/c/vr32.c",
    ];

    for source in &sources {
        build.file(source);
    }

    build.compile("libsoxr.a");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os.as_str() != "windows" {
        println!("cargo:rustc-link-lib=m");
    }
}
