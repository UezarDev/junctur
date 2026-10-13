fn main() {
    tauri_build::build();
    // With `cuda`, the same cuBLAS delay-load junctur-core's build.rs gives the CLI: link flags only
    // reach the binaries of the crate that sets them (see that file).
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if windows && std::env::var_os("CARGO_FEATURE_CUDA").is_some() {
        println!("cargo:rustc-link-arg=/DELAYLOAD:cublas64_13.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
    }
}
