extern crate rustc_version;
use {
    rustc_version::{version_meta, Channel},
    std::{env, path::PathBuf},
};

fn main() {
    // Copied and adapted from
    // https://github.com/Kimundi/rustc-version-rs/blob/1d692a965f4e48a8cb72e82cda953107c0d22f47/README.md#example
    // Licensed under Apache-2.0 + MIT
    let target_dir = env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into());

    let profile = env::var("PROFILE").unwrap();

    // Construct the path to the target directory
    #[allow(unused_variables)]
    let _target_path = if profile == "release" {
        PathBuf::from(&target_dir).join("release")
    } else {
        PathBuf::from(&target_dir).join("debug")
    };

    match version_meta().unwrap().channel {
        Channel::Stable => {
            println!("cargo:rustc-cfg=RUSTC_WITHOUT_SPECIALIZATION");
        }
        Channel::Beta => {
            println!("cargo:rustc-cfg=RUSTC_WITHOUT_SPECIALIZATION");
        }
        Channel::Nightly => {
            println!("cargo:rustc-cfg=RUSTC_WITH_SPECIALIZATION");
        }
        Channel::Dev => {
            println!("cargo:rustc-cfg=RUSTC_WITH_SPECIALIZATION");
        }
    }
    #[cfg(feature = "build_validator")]
    {
        println!("cargo:rustc-link-search=native={}", _target_path.display());
        println!("cargo:rustc-link-lib=rakurai_scheduler_1_0");
    }
}
