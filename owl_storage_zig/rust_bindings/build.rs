use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Get the directory containing the Zig project
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let zig_dir = PathBuf::from(&manifest_dir).parent().unwrap().to_path_buf();

    println!("cargo:rerun-if-changed=../src");
    println!("cargo:rerun-if-changed=../build.zig");

    // Build the Zig library
    let status = Command::new("zig")
        .args(&["build", "-Doptimize=ReleaseFast"])
        .current_dir(&zig_dir)
        .status()
        .expect("Failed to build Zig library");

    if !status.success() {
        panic!("Zig build failed");
    }

    // Link the Zig library
    let lib_dir = zig_dir.join("zig-out/lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=owl_storage");
    
    // Tell cargo to rerun if library changes
    println!("cargo:rerun-if-changed={}", lib_dir.join("libowl_storage.a").display());

    // Link system libraries
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-lib=framework=System");
    }

    #[cfg(target_os = "linux")]
    {
        println!("cargo:rustc-link-lib=c");
    }
}
