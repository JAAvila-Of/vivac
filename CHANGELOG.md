# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.2](https://github.com/JAAvila-Of/vivac/compare/v0.3.1...v0.3.2) - 2026-09-01

### Added

- *(session)* The automatic stop says what its segment held
- *(session)* Record the opening of a session in the log
- *(session)* Record what the brief claimed on each opening

### Fixed

- *(cli)* Refuse what the parser used to drop in silence
- *(session)* Refuse the Spanish spellings of the hooks
- *(session)* An opening is not a change to the tree
- *(cli)* Refuse an id that names nothing instead of hitting the focus

### Other

- *(event)* The body no longer carries the aliases it promises
- *(cli)* The help for add announces the flags it takes
- State the project's position, and close contributions for now
- Rename the Spanish identifiers d45 left behind
- Guard the identifiers, not just what the binary prints
- Correct the test count in the status section

## [0.3.1](https://github.com/JAAvila-Of/vivac/compare/v0.3.0...v0.3.1) - 2026-08-31

### Fixed

- *(brief)* A decision that governs the whole project reaches the brief

## [0.3.0](https://github.com/JAAvila-Of/vivac/compare/v0.2.1...v0.3.0) - 2026-08-31

### Added

- *(reconcile)* The diff between the tree and the anchor's history
- [**breaking**] One language. the Spanish compatibility layer is gone

### Fixed

- *(cli)* Two messages that were never translated, and derive the word list

### Other

- Cargo fmt over the sources
- The last Spanish identifiers, and none of the Spanish that carries weight

## [0.2.1](https://github.com/JAAvila-Of/vivac/compare/v0.2.0...v0.2.1) - 2026-08-31

### Fixed

- *(render)* Output that read half-translated after the rename
- *(cli)* Block --off printed half a sentence, and add the guard

## [0.2.0](https://github.com/JAAvila-Of/vivac/releases/tag/v0.2.0) - 2026-08-31

### Added

- First cut of the provenance tree in Rust
- Close Tier 0 with the brief, the vivacs and the session hooks
- *(brief)* Count what stays open under a closed node
- *(abandon)* Rescue without reparenting
- *(triage)* The pruning view the brief already named

### Fixed

- Reject an unknown option instead of ignoring it
- *(brief)* A standing decision is not an open front
- *(session)* One stop per turn is not a stop

### Other

- Add the LICENSE-APACHE the Cargo.toml already promised
- Pin line endings to LF with .gitattributes
- Update the test count after the two brief fixes
- Point the repository at the account that will host it
- Install from the registry, not from source
- Move every public string and comment to English
- Rename every identifier, keeping the old logs readable
- The README and the pillars in English
- Mark new_empty as used; mod common compiles once per binary
- The redaction guard advice, which the prose pass missed
