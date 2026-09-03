# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.6](https://github.com/JAAvila-Of/vivac/compare/v0.3.5...v0.3.6) - 2026-09-03

### Added

- *(web)* today, the page that says what moved while you were away
- *(changes)* measure a stretch from the last stop made by hand

### Fixed

- *(triage)* count the depth to a node's goal, not to the root

## [0.3.5](https://github.com/JAAvila-Of/vivac/compare/v0.3.4...v0.3.5) - 2026-09-03

### Added

- *(changes)* what a stretch of work moved, since a stop
- *(web)* the tree in a browser, on this machine and nowhere else

### Fixed

- *(cli)* check the flags before the commands that return early

### Other

- *(pillars)* add UX as the fourth pillar, with a burden of proof
- *(project)* move the long-lived tree out of the mcp server

## [0.3.4](https://github.com/JAAvila-Of/vivac/compare/v0.3.3...v0.3.4) - 2026-09-03

### Other

- state the thesis as one map, not two records side by side ([#12](https://github.com/JAAvila-Of/vivac/pull/12))

## [0.3.3](https://github.com/JAAvila-Of/vivac/compare/v0.3.2...v0.3.3) - 2026-09-02

### Added

- *(mcp)* the reads as tools an agent can call
- *(find)* text search over every field that carries meaning

### Other

- *(ops)* return what happened instead of printing it
- match the changelog to the shape the machine writes
- point the readme at the pipeline that now exists
- derive the version and publish from the commit log
- record the releases up to 0.3.2 in a changelog
- ignore the bytecode the commit guard leaves behind ([#3](https://github.com/JAAvila-Of/vivac/pull/3))
- run the suite, the linters and the guards on every pull request

## [0.3.2](https://github.com/JAAvila-Of/vivac/compare/v0.3.1...v0.3.2) - 2026-09-01

### Added

- *(session)* the automatic stop says what its segment held
- *(session)* record the opening of a session in the log
- *(session)* record what the brief claimed on each opening

### Fixed

- *(cli)* refuse what the parser used to drop in silence
- *(session)* refuse the Spanish spellings of the hooks
- *(session)* an opening is not a change to the tree
- *(cli)* refuse an id that names nothing instead of hitting the focus

### Other

- *(event)* the body no longer carries the aliases it promises
- *(cli)* the help for add announces the flags it takes
- state the project's position, and close contributions for now
- rename the Spanish identifiers d45 left behind
- guard the identifiers, not just what the binary prints
- correct the test count in the status section

## [0.3.1](https://github.com/JAAvila-Of/vivac/compare/v0.3.0...v0.3.1) - 2026-08-31

### Fixed

- *(brief)* a decision that governs the whole project reaches the brief

## [0.3.0](https://github.com/JAAvila-Of/vivac/compare/v0.2.1...v0.3.0) - 2026-08-31

### Added

- *(reconcile)* the diff between the tree and the anchor's history
- [**breaking**] one language. the Spanish compatibility layer is gone

### Fixed

- *(cli)* two messages that were never translated, and derive the word list

### Other

- cargo fmt over the sources
- the last Spanish identifiers, and none of the Spanish that carries weight

## [0.2.1](https://github.com/JAAvila-Of/vivac/compare/v0.2.0...v0.2.1) - 2026-08-31

### Fixed

- *(render)* output that read half-translated after the rename
- *(cli)* block --off printed half a sentence, and add the guard

## [0.2.0](https://github.com/JAAvila-Of/vivac/releases/tag/v0.2.0) - 2026-08-31

### Added

- first cut of the provenance tree in Rust
- close Tier 0 with the brief, the vivacs and the session hooks
- *(brief)* count what stays open under a closed node
- *(abandon)* rescue without reparenting
- *(triage)* the pruning view the brief already named

### Fixed

- reject an unknown option instead of ignoring it
- *(brief)* a standing decision is not an open front
- *(session)* one stop per turn is not a stop

### Other

- add the LICENSE-APACHE the Cargo.toml already promised
- pin line endings to LF with .gitattributes
- update the test count after the two brief fixes
- point the repository at the account that will host it
- install from the registry, not from source
- move every public string and comment to English
- rename every identifier, keeping the old logs readable
- the README and the pillars in English
- mark new_empty as used; mod common compiles once per binary
- the redaction guard advice, which the prose pass missed
