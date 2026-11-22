const FRIDA_CORE_VERSION: &str = "17.9.5";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=ATT_FRIDA_CORE_VERSION={FRIDA_CORE_VERSION}");

    #[cfg(windows)]
    link_frida_core();
}

#[cfg(windows)]
fn link_frida_core() {
    println!("cargo:rerun-if-env-changed=FRIDA_CORE_DEVKIT_DIR");
    match std::env::var_os("FRIDA_CORE_DEVKIT_DIR") {
        Some(dir) if !dir.is_empty() => {
            println!("cargo:rustc-link-search=native={}", std::path::Path::new(&dir).display());
            println!("cargo:rustc-link-lib=static=frida-core");
        }
        _ => {
            // 与 frida-sys 0.17.2 使用的版本一致；解包后直接交给 MSVC 静态链接。
            let _ = frida_build::download_and_use_devkit("core", FRIDA_CORE_VERSION);
        }
    }
    for lib in [
        "dnsapi", "iphlpapi", "psapi", "winmm", "ws2_32", "advapi32", "crypt32", "gdi32", "kernel32", "ole32",
        "secur32", "shell32", "shlwapi", "user32", "setupapi",
    ] {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }
}
