//! The README against the binary.
//!
//! Three claims went out to crates.io contradicted by the binary that shipped
//! beside them (`f161`): the README said *every read command accepts
//! `--json`* while `brief` had refused it since `d53`, it listed the web
//! interface among the things that were *not there yet* two releases after
//! the web shipped, and the maintainer's list of reads was missing two
//! commands -- which is two commands a reader cannot find out about anywhere
//! else.
//!
//! None of the three was hard to see. They were simply never read again.
//!
//! The doctrine everywhere else in this project is that the tree is the state
//! and state is never written by hand. The README is the one surface where
//! that cannot apply: crates.io renders it, so it has to be prose, and prose
//! written by hand rots. What is left is to hold it against something that
//! cannot rot, which is the binary itself -- the same move `tests/help.rs`
//! makes for the help.
//!
//! The chain has three links and every one of them is mechanical: the parser
//! holds the help honest, the help holds the README honest, and running the
//! commands holds both.

mod common;
use common::Sandbox;
use std::collections::BTreeSet;

/// Two commands are servers. `web` binds a port and opens a browser, `mcp`
/// speaks JSON-RPC until its input closes; running either here would stand a
/// server up inside the suite. For those the check goes through `--help`,
/// which `tests/help.rs` already holds against the parser.
const SERVERS: [&str; 2] = ["web", "mcp"];

fn readme() -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every fenced block, with the word that opened the fence.
fn blocks(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut tag: Option<String> = None;
    let mut body = String::new();
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("```") {
            match tag.take() {
                Some(name) => out.push((name, std::mem::take(&mut body))),
                None => tag = Some(rest.trim().to_string()),
            }
            continue;
        }
        if tag.is_some() {
            body.push_str(l);
            body.push('\n');
        }
    }
    out
}

/// The command lines of a block: what a reader would actually type.
///
/// A block tagged `sh` is all commands. Anywhere else only a `$ ` prompt
/// counts, so the rendered output the README quotes -- which is full of node
/// titles and would otherwise be read as instructions -- stays out.
fn command_lines(tag: &str, body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for l in body.lines() {
        let l = l.trim();
        let line = match l.strip_prefix("$ ") {
            Some(rest) => rest.trim(),
            None if tag == "sh" => l,
            None => continue,
        };
        if line == "vivac" || line.starts_with("vivac ") {
            out.push(cut_prose(line));
        }
    }
    out
}

/// In the reading lists a line is the command and then, a gap along, what it
/// is for. Two spaces is the separator; inside a command there are none.
fn cut_prose(line: &str) -> String {
    match line.find("  ") {
        Some(i) => line[..i].trim_end().to_string(),
        None => line.to_string(),
    }
}

/// Words, with double quotes doing what a shell would do with them.
fn split(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut started = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

fn first_word(s: &str) -> String {
    s.split_whitespace().next().unwrap_or_default().to_string()
}

/// The help for one command: its line and the indented ones under it.
fn help_block(help: &str, command: &str) -> String {
    let head = format!("vivac {command}");
    let mut inside = false;
    let mut out = String::new();
    for l in help.lines() {
        let t = l.trim_start();
        if t.starts_with("vivac ") {
            inside = t == head || t.starts_with(&format!("{head} "));
        }
        if inside {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

/// Every command the help announces, section by section.
fn all_commands(help: &str) -> BTreeSet<String> {
    help.lines()
        .filter_map(|l| l.strip_prefix("    vivac "))
        .map(first_word)
        .filter(|w| !w.is_empty())
        .collect()
}

/// The reads the help groups under the maintainer.
fn reads_from_help(help: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut inside = false;
    for l in help.lines() {
        // A section header sits two spaces in; the commands sit four.
        if !l.trim().is_empty() && l.starts_with("  ") && !l.starts_with("   ") {
            inside = l.contains("The maintainer reads");
            continue;
        }
        if inside {
            if let Some(rest) = l.strip_prefix("    vivac ") {
                out.insert(first_word(rest));
            }
        }
    }
    out
}

/// The reads the README shows the maintainer: the block under its heading.
fn reads_from_readme(text: &str) -> BTreeSet<String> {
    const MARK: &str = "**The maintainer reads.**";
    let i = text
        .find(MARK)
        .unwrap_or_else(|| panic!("the README no longer has a `{MARK}` block to check"));
    let (tag, body) = blocks(&text[i..])
        .into_iter()
        .next()
        .expect("nothing follows the maintainer's heading");
    assert_eq!(tag, "sh", "the maintainer's reads are no longer a sh block");
    command_lines(&tag, &body)
        .iter()
        .map(|command| first_word(command.trim_start_matches("vivac")))
        .filter(|w| !w.is_empty())
        .collect()
}

/// The paragraphs that talk about `--json`, run together. Fenced blocks are
/// not prose and are already checked by running them.
fn json_prose(text: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for paragraph in text.split("\n\n") {
        let opened = inside;
        if paragraph.matches("```").count() % 2 == 1 {
            inside = !inside;
        }
        if !opened && !inside && paragraph.contains("--json") {
            out.push_str(paragraph);
            out.push('\n');
        }
    }
    out
}

#[test]
fn the_binary_takes_every_command_the_readme_shows() {
    let text = readme();
    let c = Sandbox::new_seeded("readme-commands");
    let help = c.ok(&["--help"]);
    let mut seen = 0;
    for (tag, body) in blocks(&text) {
        for line in command_lines(&tag, &body) {
            let words = split(&line);
            let args: Vec<&str> = words[1..].iter().map(String::as_str).collect();
            seen += 1;
            if args.first().is_some_and(|w| SERVERS.contains(w)) {
                let block = help_block(&help, args[0]);
                assert!(
                    !block.is_empty(),
                    "the README shows `{line}`, and the help has never heard of it"
                );
                for flag in args.iter().filter(|a| a.starts_with("--")) {
                    assert!(
                        block.contains(flag),
                        "the README shows `{line}` and the help does not give it {flag}:\n{block}"
                    );
                }
                continue;
            }
            let (out, _) = c.run(&args);
            assert!(
                !out.contains("unknown command:") && !out.contains("does not take"),
                "the README shows `{line}` and the binary refuses it:\n{out}"
            );
        }
    }
    assert!(
        seen >= 10,
        "only {seen} commands found: what broke is the parsing here, not the README"
    );
}

#[test]
fn the_readme_shows_the_same_reads_as_the_help() {
    let text = readme();
    let c = Sandbox::new_seeded("readme-reads");
    let help = c.ok(&["--help"]);
    let shown = reads_from_readme(&text);
    let real = reads_from_help(&help);
    let missing: Vec<_> = real.difference(&shown).collect();
    let surplus: Vec<_> = shown.difference(&real).collect();
    assert!(
        missing.is_empty() && surplus.is_empty(),
        "the README's reading list and the help disagree.\n  \
         the help has it and the README does not: {missing:?}\n  \
         the README has it and the help does not: {surplus:?}"
    );
}

#[test]
fn the_brief_is_the_only_read_that_refuses_json() {
    let c = Sandbox::new_seeded("readme-json-run");
    let help = c.ok(&["--help"]);
    let refuses: BTreeSet<String> = reads_from_help(&help)
        .into_iter()
        .filter(|r| c.run(&[r, "--json"]).0.contains("does not take --json"))
        .collect();
    let only: BTreeSet<String> = ["brief".to_string()].into_iter().collect();
    assert_eq!(
        refuses, only,
        "the help says `--json on all of them but the brief`, and that is no longer what happens"
    );
}

#[test]
fn the_readme_names_every_read_that_refuses_json() {
    let text = readme();
    let c = Sandbox::new_seeded("readme-json-prose");
    let help = c.ok(&["--help"]);
    let prose = json_prose(&text);
    assert!(
        !prose.is_empty(),
        "the README no longer says anything about --json"
    );
    for read in reads_from_help(&help) {
        if c.run(&[&read, "--json"]).0.contains("does not take --json") {
            assert!(
                prose.contains(&read),
                "`{read}` refuses --json and the README does not say so:\n{prose}"
            );
        }
    }
}

#[test]
fn the_readme_does_not_call_missing_what_the_binary_already_does() {
    const MARK: &str = "Not there yet";
    let text = readme();
    let c = Sandbox::new_seeded("readme-missing");
    let commands = all_commands(&c.ok(&["--help"]));
    let i = text
        .find(MARK)
        .unwrap_or_else(|| panic!("the README no longer says what is `{MARK}`"));
    let rest = &text[i..];
    let sentence = &rest[..rest.find('.').unwrap_or(rest.len())];
    for word in sentence.split(|c: char| !c.is_ascii_alphanumeric()) {
        assert!(
            !commands.contains(word),
            "the README calls `{word}` missing and the binary has had it for a while:\n{sentence}"
        );
    }
}
