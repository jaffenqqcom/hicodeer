// Build script for the `assets` crate.
//
// `rust_embed`'s derive walks the asset tree while the procedural macro is
// expanded and emits one embedding macro per file it finds there
// (`rust-embed-impl`'s `get_files` loop, then `include_bytes!`/`flate!` per
// entry). The generated code depends on the files that existed at that moment
// and on nothing about the tree itself, so an asset that is added, removed, or
// renamed leaves the cached expansion valid: the crate is not rebuilt and the
// change never reaches the binary until some unrelated edit forces a
// re-expansion.
//
// Declaring the asset tree as a build-script dependency makes Cargo re-run this
// script whenever the tree changes, and Cargo scans such a directory
// recursively. The digest of the file list then changes the script's recorded
// output, which invalidates the crate and re-expands the macro. Content edits
// of a known file need no digest: their `include_bytes!` already makes them a
// dependency.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Location of the embedded asset tree relative to this crate's manifest.
const ASSET_ROOT_RELATIVE_PATH: &str = "../../assets";

/// Environment variable that carries the asset file-list digest to rustc. It is
/// not read by the crate; its only job is to change the build script's output
/// so Cargo rebuilds the crate when the tree gains or loses a file.
const ASSET_LIST_DIGEST_ENV: &str = "HICODEER_ASSETS_LIST_DIGEST";

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set for build scripts"),
    );
    let asset_root = manifest_dir.join(ASSET_ROOT_RELATIVE_PATH);
    let asset_root = fs::canonicalize(&asset_root).unwrap_or_else(|error| {
        panic!(
            "cannot resolve the asset tree at {}: {error}",
            asset_root.display()
        )
    });

    println!("cargo:rerun-if-changed={}", asset_root.display());

    let digest = digest_asset_list(&asset_root);
    println!("cargo:rustc-env={}={digest:016x}", ASSET_LIST_DIGEST_ENV);
}

/// Digest of the sorted, root-relative paths of every entry under `root`. The
/// value itself is meaningless; what matters is that adding, removing, or
/// renaming a file changes it.
fn digest_asset_list(root: &Path) -> u64 {
    let mut relative_paths = Vec::new();
    collect_relative_paths(root, root, &mut relative_paths);
    relative_paths.sort_unstable();

    let mut hasher = DefaultHasher::new();
    relative_paths.hash(&mut hasher);
    hasher.finish()
}

fn collect_relative_paths(root: &Path, directory: &Path, relative_paths: &mut Vec<String>) {
    let read_dir = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()));

    for entry in read_dir {
        let entry = entry.unwrap_or_else(|error| {
            panic!("cannot read an entry under {}: {error}", directory.display())
        });
        let path = entry.path();
        let file_type = entry
            .file_type()
            .unwrap_or_else(|error| panic!("cannot stat {}: {error}", path.display()));

        if file_type.is_dir() {
            collect_relative_paths(root, &path, relative_paths);
            continue;
        }

        let relative_path = path
            .strip_prefix(root)
            .expect("every walked path starts with the asset root")
            .to_string_lossy()
            .into_owned();
        relative_paths.push(relative_path);
    }
}
