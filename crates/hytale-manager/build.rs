//! `TARGET` is set for build scripts only, and `hy self update` needs the exact triple to
//! pick a release asset — `std::env::consts` cannot tell musl from gnu.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!(
        "cargo::rustc-env=HY_TARGET={}",
        std::env::var("TARGET").expect("cargo sets TARGET for build scripts")
    );
}
