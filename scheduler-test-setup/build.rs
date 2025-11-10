extern crate rustc_version;
use {
    rustc_version::{version_meta, Channel},
    std::{env, path::PathBuf},
};

#[allow(unused_imports)]
use std::fs;

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

    #[cfg(feature = "local_build")]
    {
        println!("cargo:rustc-link-search=native={}", _target_path.display());
        println!("cargo:rustc-link-lib=rakurai_scheduler_1_0");
    }
    #[cfg(not(feature = "local_build"))]
    {
        #[cfg(feature = "build_validator")]
        {
            // Get version from CARGO_PKG_VERSION (e.g., "3.0.14")
            let version =
                env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION should be set by Cargo");

            // Parse version components
            let version_parts: Vec<&str> = version.split('.').collect();
            let major = version_parts.get(0).unwrap_or(&"0");
            let minor = version_parts.get(1).unwrap_or(&"0");
            let patch = version_parts.get(2).unwrap_or(&"0");

            // Read scheduler_version from root Cargo.toml
            let manifest_dir =
                env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set by Cargo");
            let root_cargo_toml = PathBuf::from(&manifest_dir)
                .parent()
                .map(|p| p.join("Cargo.toml"))
                .expect("Could not find root Cargo.toml");

            let cargo_toml_content =
                fs::read_to_string(&root_cargo_toml).expect("Failed to read root Cargo.toml");

            // Parse scheduler_version from [workspace.metadata.build] section
            let scheduler_version = cargo_toml_content
                .lines()
                .skip_while(|line| !line.contains("[workspace.metadata.build]"))
                .skip(1)
                .take_while(|line| {
                    let trimmed = line.trim();
                    trimmed.is_empty() || !trimmed.starts_with('[')
                })
                .find_map(|line| {
                    let trimmed = line.trim();
                    if trimmed.starts_with("scheduler_version") && !trimmed.starts_with('#') {
                        trimmed.split('=').nth(1).and_then(|s| {
                            s.trim()
                                .trim_matches('"')
                                .trim_matches('\'')
                                .parse::<u32>()
                                .ok()
                        })
                    } else {
                        None
                    }
                })
                .unwrap_or(1);

            // Construct library name: rak_scheduler_{major}_{minor}_{patch}_{scheduler_version}
            let lib_name = format!(
                "rak_scheduler_{}_{}_{}_{}",
                major, minor, patch, scheduler_version
            );

            println!("cargo:rustc-link-search=native={}", _target_path.display());
            println!("cargo:rustc-link-lib={}", lib_name);
        }
    }
}
