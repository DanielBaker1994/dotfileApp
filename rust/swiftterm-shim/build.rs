// swiftterm-shim build script (PLAN-rust-port.md, Phase 0.7).
//
// Compiles `shim/WSShim.swift` against the pinned SwiftTerm module and links
// the resulting object + `.build/SwiftTerm/libSwiftTerm.a` for dependents.
// The SwiftTerm checkout is read-only: never built or modified here. When the
// checkout or its static lib is absent this is a no-op so the workspace still
// builds (terminal support is simply disabled).
use std::path::{Path, PathBuf};
use std::process::Command;

fn read_conf(repo: &Path, key: &str, default: &str) -> String {
    let Ok(text) = std::fs::read_to_string(repo.join("install.conf")) else {
        return default.to_string();
    };
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            if let Some(v) = rest.strip_prefix('=') {
                return v.trim().trim_matches('"').to_string();
            }
        }
    }
    default.to_string()
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=shim/WSShim.swift");
    println!("cargo:rerun-if-env-changed=SWIFTTERM_DIR");
    println!("cargo:rustc-check-cfg=cfg(swiftterm_present)");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo = manifest
        .parent()
        .and_then(Path::parent)
        .expect("swiftterm-shim lives at rust/swiftterm-shim")
        .to_path_buf();

    let swiftterm_dir = {
        let conf = std::env::var("SWIFTTERM_DIR")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| read_conf(&repo, "SWIFTTERM_DIR", "../SwiftTerm"));
        let p = PathBuf::from(&conf);
        if p.is_absolute() {
            p
        } else {
            repo.join(p)
        }
    };
    let module_dir = repo.join(".build/SwiftTerm");
    let term_lib = module_dir.join("libSwiftTerm.a");

    println!("cargo:rerun-if-changed={}", repo.join("install.conf").display());
    println!("cargo:rerun-if-changed={}", swiftterm_dir.join(".ws-pinned").display());
    println!("cargo:rerun-if-changed={}", term_lib.display());

    if !swiftterm_dir.exists() || !term_lib.exists() {
        println!(
            "cargo:warning=SwiftTerm not found at {} ({}) — swiftterm-shim builds as a no-op; the terminal is disabled",
            swiftterm_dir.display(),
            term_lib.display()
        );
        return;
    }

    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    let macos_min = read_conf(&repo, "MACOS_MIN", "26.7");
    let target = format!("{arch}-apple-macosx{macos_min}");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let obj = out_dir.join("WSShim.o");
    let shim = manifest.join("shim/WSShim.swift");

    let status = Command::new("swiftc")
        .args(["-O", "-swift-version", "5"])
        .arg("-target")
        .arg(&target)
        .args(["-parse-as-library", "-emit-object"])
        .arg("-I")
        .arg(&module_dir)
        .arg("-o")
        .arg(&obj)
        .arg(&shim)
        .status()
        .expect("failed to run swiftc (install the Xcode command line tools)");
    if !status.success() {
        panic!("swiftc failed to compile {}", shim.display());
    }

    // Archive the shim object so the whole crate links for dependents through
    // rustc-link-lib (a rustc-link-arg would not propagate downstream).
    let shim_lib = out_dir.join("libWSShim.a");
    let archive = Command::new("libtool")
        .args(["-static", "-o"])
        .arg(&shim_lib)
        .arg(&obj)
        .status();
    let archived = matches!(archive, Ok(s) if s.success());
    if !archived {
        let ar = Command::new("ar")
            .arg("crs")
            .arg(&shim_lib)
            .arg(&obj)
            .status()
            .expect("failed to run libtool or ar");
        assert!(ar.success(), "failed to archive {}", obj.display());
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=WSShim");
    println!("cargo:rustc-link-search=native={}", module_dir.display());
    println!("cargo:rustc-link-lib=static=SwiftTerm");
    for framework in ["AppKit", "CoreText", "Metal", "MetalKit", "QuartzCore"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }

    println!("cargo:rustc-cfg=swiftterm_present");
}
