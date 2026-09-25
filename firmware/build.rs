use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rustc-link-arg-bins=-Tlinkall.x");
    println!("cargo:rerun-if-changed=memory.x");
    patch_esp_hal_memory_x();
}

/// Replace the `memory.x` esp-hal puts on the linker search path with ours.
///
/// esp-hal's `ld/<chip>/memory.x` hard-codes 4 MB flash windows, which is not
/// enough for the sprite/cry/BGM blobs (the chip has 128 x 64 KB of flash MMU
/// pages shared between the two windows, i.e. 8 MB of mapped flash — see
/// `firmware/memory.x`). Shipping our own `memory.x` next to the crate is not
/// enough: esp-hal's build script drops its copy into its own OUT_DIR and puts
/// that directory *ahead* of ours on the `-L` list, so `INCLUDE "memory.x"`
/// inside `linkall.x` always finds esp-hal's first. Overwriting the copy the
/// linker actually reads is the only reliable override.
fn patch_esp_hal_memory_x() {
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    // <target>/<profile>/build/<crate>-<hash>/out -> <target>/<profile>/build
    let Some(build_dir) = out.ancestors().nth(2) else {
        return;
    };
    let ours = Path::new(env!("CARGO_MANIFEST_DIR")).join("memory.x");
    let mut patched = 0;
    let Ok(entries) = fs::read_dir(build_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("esp-hal-")
        {
            continue;
        }
        let target = entry.path().join("out").join("memory.x");
        if target.exists() {
            fs::copy(&ours, &target).expect("overwrite esp-hal memory.x");
            patched += 1;
        }
    }
    if patched == 0 {
        println!(
            "cargo:warning=memory.x override did not find esp-hal's copy; \
             the link is likely to fail with a DROM overflow"
        );
    }
}
