# Reviewed dependency notices

The release notice generator normally reads license and notice files from each
linked crate. Two pinned crates omit those files from their published packages.
Their reviewed notices are kept here so packaging can include complete notices
without fetching mutable license text during a build.

`../third-party-notices.json` binds each reviewed entry to its exact crate name,
version, crates.io source and declared license expression. It records the source
archive checksum and license provenance. Cargo's locked dependency resolution
verifies the registry archive; the notice generator additionally verifies every
local notice file's SHA-256. The archive checksum in this manifest documents
that review and does not replace Cargo's integrity checks.

An unrecognized package, version, source or license expression does not receive
these notices. Missing or changed reviewed files fail packaging. A crate's own
license files take priority when present. Dependency upgrades require reviewing
their notices again; this is not a generic license-text substitution.

## hex-slice 0.1.4

The published crate declares MIT but contains no license file. Its original
repository is no longer available at the URL in its package metadata.

The [official Debian source package](https://packages.debian.org/source/stable/rust-hex-slice)
for `rust-hex-slice 0.1.4-1` supplies an
[upstream copyright attribution and complete MIT license](https://metadata.ftp-master.debian.org/changelogs//main/r/rust-hex-slice/rust-hex-slice_0.1.4-1_copyright).
The original archive's MD5 matches the Debian source listing, the registry
archive's SHA-256 matches Cargo.lock, and all eight source files in those two
archives match individually by SHA-256. This establishes which source the
Debian attribution covers; a fork's license was not substituted.

The retained notice preserves the `Files: *` copyright line and the MIT license
paragraph. Debian paragraph indentation is removed and its single-dot blank
lines become ordinary blank lines. Debian packaging-only attributions are not
included. Source-document and resulting-notice hashes are in the manifest.

## mp4ra-rust 0.6.0

The package declares `MIT/Apache-2.0`. Its packaged `.cargo_vcs_info.json`
identifies commit `3fc2d3a508b7867f3aa981ca902b24260fb0223f` and the
`mp4ra-rust` subdirectory in the upstream repository. The corresponding
[MIT](https://github.com/dholroyd/mp4ra-rust/blob/3fc2d3a508b7867f3aa981ca902b24260fb0223f/LICENSE-MIT)
and [Apache](https://github.com/dholroyd/mp4ra-rust/blob/3fc2d3a508b7867f3aa981ca902b24260fb0223f/LICENSE-APACHE)
files are at that repository's root, outside the packaged subdirectory. Both
files are retained here byte for byte at that exact commit.

Run the isolated generator checks without invoking Cargo:

```text
python tools/test-third-party-notices.py
```
