//! CUDA builds on Windows: delay-load cuBLAS, whose folder is not on PATH until
//! `runtime::prepare_cuda` adds it, so a normal import would stop the program from starting.
//! Shared by both crates (the CLI's `build` points here): link flags only reach the setting crate.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows && std::env::var_os("CARGO_FEATURE_CUDA").is_some() {
        println!("cargo:rustc-link-arg=/DELAYLOAD:cublas64_13.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
    }
}
