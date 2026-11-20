fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // GPUI's debug-build element construction exceeds the MSVC default
        // main-thread stack while rendering the workspace.
        println!("cargo:rustc-link-arg=/STACK:8388608");
    }
}
