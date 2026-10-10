//! CUDA builds on Windows: delay-load the cuBLAS DLL that whisper.cpp links against (it links the
//! CUDA runtime statically, so cuBLAS is the only toolkit DLL the binary imports).
//!
//! The CUDA 13 installer puts its DLLs in `bin\x64`, which is not on PATH, and a normal import
//! would stop the program from starting at all. Delay-loaded, they resolve on first use, after
//! `runtime::prepare_cuda` has put the CUDA folder on PATH. Shared by both crates (the CLI's
//! `build` points here) because link flags only reach the binaries of the crate that sets them.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows && std::env::var_os("CARGO_FEATURE_CUDA").is_some() {
        println!("cargo:rustc-link-arg=/DELAYLOAD:cublas64_13.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
    }
}
