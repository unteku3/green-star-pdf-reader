use std::{env, path::PathBuf, process::Command};

fn run(command: &mut Command) {
    let status = command
        .status()
        .expect("Could not run the system C toolchain");
    assert!(status.success(), "System C toolchain failed: {command:?}");
}

fn main() {
    println!("cargo:rerun-if-changed=native/pdfkit.m");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo did not set OUT_DIR"));
    let object = output.join("pdfkit.o");
    run(Command::new("xcrun")
        .args([
            "--sdk",
            "macosx",
            "clang",
            "-fobjc-arc",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-target",
            "arm64-apple-macos11",
            "-c",
            "native/pdfkit.m",
            "-o",
        ])
        .arg(&object));
    run(Command::new("xcrun")
        .args(["ar", "crs"])
        .arg(output.join("libpdfkit_bridge.a"))
        .arg(object));
    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=pdfkit_bridge");
    println!("cargo:rustc-link-lib=framework=PDFKit");
    println!("cargo:rustc-link-lib=framework=Foundation");
}
