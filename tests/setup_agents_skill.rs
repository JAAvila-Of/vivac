mod common;
use common::Sandbox;
use std::path::{Path, PathBuf};

const HARNESSES: [(&str, &str); 2] = [
    ("claude-code", ".claude/skills/vivac-agents/SKILL.md"),
    ("codex", ".agents/skills/vivac-agents/SKILL.md"),
];

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
        if !path.exists() {
            return;
        }
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_symlink() {
                files.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read_link(&path)
                        .unwrap()
                        .to_string_lossy()
                        .as_bytes()
                        .to_vec(),
                ));
            } else if kind.is_dir() {
                files.push((path.strip_prefix(root).unwrap().to_path_buf(), vec![]));
                visit(root, &path, files);
            } else {
                files.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                ));
            }
        }
    }
    let mut files = vec![];
    visit(root, root, &mut files);
    files.sort();
    files
}

fn put(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn previous_copy(name: &str) -> String {
    let frontmatter = format!("---\nname: {name}\ndescription: An earlier skill.\n---\n");
    let body = "\nEarlier instructions.\n";
    let content = format!("{frontmatter}{body}");
    let mut fingerprint = 0xcbf29ce484222325u64;
    for byte in content.bytes() {
        fingerprint = (fingerprint ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    format!("{frontmatter}<!-- written by vivac setup; fingerprint {fingerprint:016x}; setup removes it with --undo while the text is unchanged -->\n{body}")
}

#[test]
fn setup_and_undo_dry_runs_leave_project_and_registry_unchanged() {
    for (harness, label) in HARNESSES {
        let c = Sandbox::new_seeded("agents-dry-run");
        for installed in [false, true] {
            if installed {
                c.ok(&["setup", harness, "--yes"]);
            }
            let before = snapshot(&c.0);
            let registry = snapshot(c.global_home());
            let output = c.ok(&["setup", harness, "--dry-run"]);
            assert!(output.contains(label), "{output}");
            c.ok(&["setup", harness, "--undo", "--dry-run"]);
            assert_eq!(snapshot(&c.0), before);
            assert_eq!(snapshot(c.global_home()), registry);
        }
    }
}

#[test]
fn idempotence_preserves_bytes_and_missing_skill_is_reinstalled() {
    for (harness, label) in HARNESSES {
        let c = Sandbox::new_seeded("agents-idempotent");
        c.ok(&["setup", harness, "--yes"]);
        let before = snapshot(&c.0);
        c.ok(&["setup", harness, "--yes"]);
        assert_eq!(snapshot(&c.0), before);
        std::fs::remove_file(c.0.join(label)).unwrap();
        c.ok(&["setup", harness, "--yes"]);
        assert_eq!(snapshot(&c.0), before);
    }
}

#[test]
fn unchanged_previous_copy_is_upgraded_but_edited_copy_blocks_all_writes() {
    for (harness, label) in HARNESSES {
        let c = Sandbox::new_seeded("agents-upgrade");
        let path = c.0.join(label);
        let previous = previous_copy("vivac-agents");
        put(&path, previous.as_bytes());
        let preview = c.ok(&["setup", harness, "--dry-run"]);
        assert!(preview.contains("replace"), "{preview}");
        c.ok(&["setup", harness, "--yes"]);
        assert_ne!(std::fs::read_to_string(&path).unwrap(), previous);

        let c = Sandbox::new_seeded("agents-edited");
        put(
            &c.0.join(label),
            format!("{previous}A local edit.\n").as_bytes(),
        );
        let before = snapshot(&c.0);
        let registry = snapshot(c.global_home());
        let (output, code) = c.run(&["setup", harness, "--yes"]);
        assert_ne!(code, 0, "{output}");
        assert!(output.contains(label), "{output}");
        assert_eq!(snapshot(&c.0), before);
        assert_eq!(snapshot(c.global_home()), registry);
    }
}

#[test]
fn foreign_unmarked_and_invalid_utf8_skills_are_preserved_without_writes() {
    for (harness, label) in HARNESSES {
        for bytes in [
            b"---\nname: vivac-agents\n---\nLocal instructions.\n".to_vec(),
            previous_copy("vivac-migrate").into_bytes(),
            vec![0xff, 0xfe],
        ] {
            let c = Sandbox::new_seeded("agents-foreign");
            put(&c.0.join(label), &bytes);
            let before = snapshot(&c.0);
            let (output, code) = c.run(&["setup", harness, "--yes"]);
            assert_ne!(code, 0, "{output}");
            assert_eq!(snapshot(&c.0), before);
            c.ok(&["setup", harness, "--undo", "--yes"]);
            assert_eq!(snapshot(&c.0), before);
        }
    }
}

#[test]
fn undo_removes_only_own_unchanged_skill_and_keeps_the_tree_and_other_harness() {
    for (harness, label) in HARNESSES {
        let c = Sandbox::new_seeded("agents-undo");
        for (other, _) in HARNESSES {
            c.ok(&["setup", other, "--yes"]);
        }
        let tree = snapshot(&c.0.join(".vivac"));
        let other_label = HARNESSES
            .iter()
            .find(|(other, _)| *other != harness)
            .unwrap()
            .1;
        let other = std::fs::read(c.0.join(other_label)).unwrap();
        c.ok(&["setup", harness, "--undo", "--yes"]);
        assert!(!c.0.join(label).exists());
        assert!(!c.0.join(label).parent().unwrap().exists());
        assert_eq!(std::fs::read(c.0.join(other_label)).unwrap(), other);
        assert_eq!(snapshot(&c.0.join(".vivac")), tree);

        c.ok(&["setup", harness, "--yes"]);
        let path = c.0.join(label);
        let mut edited = std::fs::read(&path).unwrap();
        edited.extend_from_slice(b"\nA local edit.\n");
        put(&path, &edited);
        c.ok(&["setup", harness, "--undo", "--yes"]);
        assert_eq!(std::fs::read(path).unwrap(), edited);
        assert_eq!(snapshot(&c.0.join(".vivac")), tree);
    }
}

#[test]
fn directories_in_place_of_files_and_files_in_place_of_directories_block_setup() {
    for (harness, label) in HARNESSES {
        for directory in [false, true] {
            let c = Sandbox::new_seeded("agents-wrong-type");
            let path = c.0.join(label);
            if directory {
                std::fs::create_dir_all(path).unwrap();
            } else {
                put(path.parent().unwrap(), b"Not a directory.\n");
            }
            let before = snapshot(&c.0);
            let (output, code) = c.run(&["setup", harness, "--yes"]);
            assert_ne!(code, 0, "{output}");
            assert_eq!(snapshot(&c.0), before);
        }
    }
}

#[test]
fn undo_accepts_previous_unchanged_copy_and_keeps_nonempty_skill_directory() {
    for (harness, label) in HARNESSES {
        let c = Sandbox::new_seeded("agents-old-undo");
        let path = c.0.join(label);
        put(&path, previous_copy("vivac-agents").as_bytes());
        let other = path.parent().unwrap().join("notes.txt");
        put(&other, b"Local notes.\n");
        let tree = snapshot(&c.0.join(".vivac"));
        c.ok(&["setup", harness, "--undo", "--yes"]);
        assert!(!path.exists());
        assert_eq!(std::fs::read(other).unwrap(), b"Local notes.\n");
        assert_eq!(snapshot(&c.0.join(".vivac")), tree);
    }
}

#[cfg(windows)]
#[test]
fn unreadable_skill_blocks_setup_and_is_preserved_by_undo() {
    use std::os::windows::fs::OpenOptionsExt;
    for (harness, label) in HARNESSES {
        let c = Sandbox::new_seeded("agents-unreadable");
        let path = c.0.join(label);
        put(&path, previous_copy("vivac-agents").as_bytes());
        let before = snapshot(&c.0);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let (output, code) = c.run(&["setup", harness, "--yes"]);
        assert_ne!(code, 0, "{output}");
        c.ok(&["setup", harness, "--undo", "--yes"]);
        drop(file);
        assert_eq!(snapshot(&c.0), before);
    }
}

#[cfg(windows)]
#[test]
fn parent_junction_is_never_followed_or_removed() {
    for (harness, label) in HARNESSES {
        let c = Sandbox::new_seeded("agents-junction");
        let destination = c.0.join("foreign");
        std::fs::create_dir(&destination).unwrap();
        put(
            &destination.join("SKILL.md"),
            previous_copy("vivac-agents").as_bytes(),
        );
        let path = c.0.join(label);
        let link = path.parent().unwrap();
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link.to_string_lossy().replace('/', "\\"))
            .arg(destination.to_string_lossy().replace('/', "\\"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let before = snapshot(&c.0);
        let (output, code) = c.run(&["setup", harness, "--yes"]);
        assert_ne!(code, 0, "{output}");
        assert_eq!(snapshot(&c.0), before);
        c.ok(&["setup", harness, "--undo", "--yes"]);
        assert_eq!(snapshot(&c.0), before);
    }
}

#[cfg(unix)]
#[test]
fn file_and_parent_symlinks_are_never_followed_or_removed() {
    for (harness, label) in HARNESSES {
        for parent in [false, true] {
            let c = Sandbox::new_seeded("agents-links");
            let destination = c.0.join("foreign");
            std::fs::create_dir(&destination).unwrap();
            put(
                &destination.join("SKILL.md"),
                previous_copy("vivac-agents").as_bytes(),
            );
            let path = c.0.join(label);
            let link = if parent {
                path.parent().unwrap()
            } else {
                &path
            };
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            let target = if parent {
                destination.clone()
            } else {
                destination.join("SKILL.md")
            };
            std::os::unix::fs::symlink(target, link).unwrap();
            let before = snapshot(&c.0);
            let (output, code) = c.run(&["setup", harness, "--yes"]);
            assert_ne!(code, 0, "{output}");
            assert_eq!(snapshot(&c.0), before);
            c.ok(&["setup", harness, "--undo", "--yes"]);
            assert_eq!(snapshot(&c.0), before);
        }
    }
}

#[test]
fn both_harnesses_install_the_same_agents_skill_without_agents() {
    let c = Sandbox::new_seeded("setup-agents-skill");
    c.ok(&["setup", "claude-code", "--yes"]);
    let claude = std::fs::read(c.0.join(".claude/skills/vivac-agents/SKILL.md")).unwrap();
    c.ok(&["setup", "codex", "--yes"]);
    let codex = std::fs::read(c.0.join(".agents/skills/vivac-agents/SKILL.md")).unwrap();
    assert_eq!(claude, codex);
    let text = String::from_utf8(claude).unwrap();
    assert!(text.starts_with("---\nname: vivac-agents\n"));
    let mut lines: Vec<_> = text.split('\n').collect();
    let close = lines
        .iter()
        .skip(1)
        .position(|line| *line == "---")
        .unwrap()
        + 1;
    assert!(lines[close + 1].starts_with("<!-- written by vivac setup; fingerprint "));
    lines.remove(close + 1);
    assert_eq!(
        lines.join("\n"),
        include_str!("../src/setup/agents-skill.md")
    );
    assert!(!c.0.join(".claude/agents").exists());
    assert!(!c.0.join(".codex/agents").exists());
}
