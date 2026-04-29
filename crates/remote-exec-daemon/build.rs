fn main() {
    // Rust 1.85's built-in cfg validation does not know the later-added Cygwin target,
    // but we still want to keep the source-level branch compiled without warning.
    println!("cargo:rustc-check-cfg=cfg(target_os, values(\"cygwin\"))");
}
