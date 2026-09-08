//! Stamps the build conditions into the binary so no number can be quoted
//! without them.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rustc-env=CASE_TARGET={}",
        std::env::var("TARGET").unwrap_or_else(|_| "unknown".into())
    );
    println!(
        "cargo:rustc-env=CASE_PROFILE={}",
        std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into())
    );
    println!(
        "cargo:rustc-env=CASE_OPT_LEVEL={}",
        std::env::var("OPT_LEVEL").unwrap_or_else(|_| "unknown".into())
    );
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=CASE_RUSTC={}", version.trim());
    println!(
        "cargo:rustc-env=CASE_RUSTFLAGS={}",
        std::env::var("CARGO_ENCODED_RUSTFLAGS")
            .unwrap_or_default()
            .replace('\u{1f}', " ")
    );
}
