//! Records the SHA-256 of the driver catalog shipped with this build.
//!
//! The elevated installer step signs the catalog on each PC, and it only
//! signs a catalog whose hash matches this value.

use std::path::PathBuf;

use sha2::{Digest, Sha256};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let cat = manifest.join("../../drivers/serialloom-port/target/release/package/serialloom_port.cat");
    println!("cargo:rerun-if-changed={}", cat.display());

    let hash = match std::fs::read(&cat) {
        Ok(bytes) => format!("{:x}", Sha256::digest(&bytes)),
        Err(_) => {
            println!(
                "cargo:warning=driver package not found; this build cannot install the driver. Run drivers/serialloom-port/package.ps1 -NoSign first."
            );
            String::new()
        }
    };
    println!("cargo:rustc-env=SERIALLOOM_DRIVER_CAT_SHA256={hash}");
}
