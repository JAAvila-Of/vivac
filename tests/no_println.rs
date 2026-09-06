//! Bans `println!` under `src/`.
//!
//! `println!` writes through `Stdout`'s `LineWriter`, which flushes -- one
//! syscall -- on every newline. That was most of why `tree` broke its own
//! 50 ms budget at 10 000 nodes: composing 6845 lines cost a fraction of what
//! handing them to the terminal one at a time did. `src/output.rs`'s `outln!`
//! buffers instead and `main` flushes it once, on every path out. Without a
//! guard, the next line-by-line render brings the same defect straight back.
//!
//! Same shape as `tests/identifiers.rs`: strip comments and string literals
//! with `literal_spans` before searching, so a doc comment that talks about
//! `println!` -- `src/mcp.rs` has one, explaining why its own transport
//! cannot use it -- does not trip the guard. And a match only counts if the
//! six letters in front of it are not `eprintln!`'s own: a plain substring
//! search would flag every one of those on stderr, which this guard has no
//! quarrel with.

#[path = "common/literal_spans.rs"]
mod literal_spans;

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `src`, all the way down. Same walk
/// `tests/identifiers.rs` uses, narrowed to `src`: the ban is on the crate's
/// own code, not on the tests that check it.
fn sources() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut pending: Vec<std::path::PathBuf> = vec![root().join("src")];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                pending.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// The source with every comment and every literal removed, so a mention of
/// `println!` in prose or in a quoted example never counts as a call.
fn code_only(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut drop = vec![false; chars.len()];
    for (a, e) in literal_spans::literal_spans(src) {
        drop[a..e].fill(true);
    }
    for (a, e) in literal_spans::comment_spans(src) {
        drop[a..e].fill(true);
    }
    chars
        .iter()
        .zip(drop)
        .filter(|(_, d)| !*d)
        .map(|(c, _)| *c)
        .collect()
}

/// `println!` sites in `code`, `eprintln!` excluded even though it ends in
/// the same six letters and a plain substring search would find it. A hit
/// only counts when the character right in front of it cannot extend the
/// word the other way -- letting `eprintln!` through and nothing else.
fn bare_println_sites(code: &str) -> usize {
    code.match_indices("println!")
        .filter(|(i, _)| match code[..*i].chars().next_back() {
            Some(c) => !(c.is_ascii_alphanumeric() || c == '_'),
            None => true,
        })
        .count()
}

#[test]
fn no_println_under_src() {
    let offenders: Vec<(String, usize)> = sources()
        .into_iter()
        .filter_map(|p| {
            let code = code_only(&std::fs::read_to_string(&p).unwrap());
            let n = bare_println_sites(&code);
            if n == 0 {
                return None;
            }
            let where_at = p
                .strip_prefix(root())
                .unwrap_or(&p)
                .display()
                .to_string()
                .replace('\\', "/");
            Some((where_at, n))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "\n  {} file(s) under src use println!:\n\n      {}\n\n  \
         `println!` writes through `Stdout`'s `LineWriter`, which flushes -- one\n  \
         syscall -- on every line. Use `outln!` from `crate::output` instead: it\n  \
         buffers, and `main` flushes it once on every path out.\n",
        offenders.len(),
        offenders
            .iter()
            .map(|(f, n)| format!("{f} ({n})"))
            .collect::<Vec<_>>()
            .join("\n      ")
    );
}
