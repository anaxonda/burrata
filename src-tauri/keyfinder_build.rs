use std::path::PathBuf;

/// Build the pinned keyfinder, FFTW and C ABI wrapper as static archives.
/// All three are installed under Cargo's OUT_DIR and linked into the app; no
/// analyzer executable or runtime package is installed on the user's system.
pub fn build() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source = root.join("keyfinder-native");
    let installed = cmake::Config::new(&source)
        .profile("Release")
        .define("CMAKE_POLICY_VERSION_MINIMUM", "3.5")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("BUILD_TESTING", "OFF")
        .define("BUILD_TESTS", "OFF")
        .define("ENABLE_FLOAT", "OFF")
        .define("ENABLE_LONG_DOUBLE", "OFF")
        .define("ENABLE_QUAD_PRECISION", "OFF")
        .define("DISABLE_FORTRAN", "ON")
        .define("ENABLE_THREADS", "OFF")
        .define("ENABLE_OPENMP", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .build();

    let lib = installed.join("lib");
    for name in ["burrata_keyfinder_ffi", "keyfinder", "fftw3"] {
        println!("cargo:rustc-link-lib=static={name}");
    }
    println!("cargo:rustc-link-search=native={}", lib.display());
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => println!("cargo:rustc-link-lib=dylib=stdc++"),
        Ok("macos") => println!("cargo:rustc-link-lib=dylib=c++"),
        _ => {}
    }
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("unix") {
        println!("cargo:rustc-link-lib=dylib=m");
    }

    println!("cargo:rerun-if-changed=keyfinder-native/CMakeLists.txt");
    println!("cargo:rerun-if-changed=keyfinder-native/keyfinder_ffi.cpp");
    println!("cargo:rerun-if-changed=vendor/libkeyfinder");
    println!("cargo:rerun-if-changed=vendor/fftw");
}
