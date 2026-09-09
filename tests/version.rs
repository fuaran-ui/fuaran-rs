//! `fuaran_rs::VERSION` against the manifest that owns the number.
//!
//! The constant was a hand-written literal, and it drifted: it read
//! `0.0.4-alpha` while `Cargo.toml` read `0.0.6-alpha.1`, because two later
//! releases bumped the manifest and nothing anywhere compared the two. It is
//! `env!("CARGO_PKG_VERSION")` now, so today the equality holds by
//! construction — which is exactly why this test reads the manifest TEXT rather
//! than the same environment variable. Comparing `VERSION` to
//! `env!("CARGO_PKG_VERSION")` would be a tautology that passes just as happily
//! after somebody writes a literal back; reading the file catches that.

/// The `version` of the manifest's `[package]` table, by a deliberately small
/// scan — this crate declares no dependencies, a TOML parser included.
fn manifest_package_version(manifest: &str) -> Option<&str> {
    let mut in_package = false;

    for line in manifest.lines() {
        let line = line.trim();

        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }

        if !in_package {
            continue;
        }

        let Some(rest) = line.strip_prefix("version") else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            continue;
        };

        return rest
            .trim()
            .strip_prefix('"')
            .and_then(|rest| rest.split('"').next());
    }

    None
}

#[test]
fn version_is_the_manifest_version() {
    let manifest = include_str!("../Cargo.toml");
    let declared = manifest_package_version(manifest)
        .expect("Cargo.toml declares no [package] version — the scan below is what to fix");

    assert_eq!(
        fuaran_rs::VERSION,
        declared,
        "fuaran_rs::VERSION and Cargo.toml disagree. The constant is derived \
         (`env!(\"CARGO_PKG_VERSION\")`); a literal has been written back over it."
    );
}

/// The scan is the load-bearing half of the test above, so prove it can go red
/// and that it reads the right table — a `[package]` version and a
/// `[dependencies]` entry's version are the same three lines apart.
#[test]
fn the_manifest_scan_reads_the_package_table_only() {
    let manifest = "\
[workspace]\n\
version = \"9.9.9\"\n\
\n\
[package]\n\
name = \"x\"\n\
version = \"1.2.3-alpha.4\"\n\
\n\
[dependencies]\n\
version = \"0.0.0\"\n";

    assert_eq!(manifest_package_version(manifest), Some("1.2.3-alpha.4"));
    assert_eq!(manifest_package_version("[package]\nname = \"x\"\n"), None);
}
