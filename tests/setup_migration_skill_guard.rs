mod common;

use common::Sandbox;
use std::path::{Path, PathBuf};

fn skill_path(c: &Sandbox) -> PathBuf {
    c.0.join(".claude/skills/vivac-migrate/SKILL.md")
}

fn file_snapshot(path: &Path) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("reading {path:?}: {error}"),
    }
}

fn assert_refused_without_changes(c: &Sandbox, undo: bool) {
    let settings = c.0.join(".claude/settings.json");
    let mcp = c.0.join(".mcp.json");
    let agents = c.0.join(".claude/skills/vivac-agents/SKILL.md");
    let before = [
        file_snapshot(&settings),
        file_snapshot(&mcp),
        file_snapshot(&agents),
    ];
    let args = if undo {
        vec!["setup", "claude-code", "--undo", "--yes"]
    } else {
        vec!["setup", "claude-code", "--yes"]
    };
    let (output, code) = c.run(&args);
    assert_eq!(code, 1, "{output}");
    assert!(output.contains("Nothing written."), "{output}");
    assert_eq!(file_snapshot(&settings), before[0]);
    assert_eq!(file_snapshot(&mcp), before[1]);
    assert_eq!(file_snapshot(&agents), before[2]);
}

#[test]
fn migration_skill_directory_refuses_apply_and_undo_before_other_writes() {
    for undo in [false, true] {
        let c = Sandbox::new_seeded("migration-skill-directory");
        if undo {
            c.ok(&["setup", "claude-code", "--yes"]);
            std::fs::remove_file(skill_path(&c)).unwrap();
        }
        std::fs::create_dir_all(skill_path(&c)).unwrap();
        let sentinel = skill_path(&c).join("preserve.txt");
        std::fs::write(&sentinel, b"synthetic directory content").unwrap();
        assert_refused_without_changes(&c, undo);
        assert_eq!(
            std::fs::read(sentinel).unwrap(),
            b"synthetic directory content"
        );
    }
}

#[test]
fn migration_skill_invalid_utf8_refuses_apply_and_undo_before_other_writes() {
    for undo in [false, true] {
        let c = Sandbox::new_seeded("migration-skill-invalid-utf8");
        if undo {
            c.ok(&["setup", "claude-code", "--yes"]);
        }
        std::fs::create_dir_all(skill_path(&c).parent().unwrap()).unwrap();
        let invalid = b"synthetic skill\xff\xfe";
        std::fs::write(skill_path(&c), invalid).unwrap();
        assert_refused_without_changes(&c, undo);
        assert_eq!(std::fs::read(skill_path(&c)).unwrap(), invalid);
    }
}

#[cfg(windows)]
#[test]
fn migration_skill_locked_file_refuses_apply_and_undo_before_other_writes() {
    use std::os::windows::fs::OpenOptionsExt;

    for undo in [false, true] {
        let c = Sandbox::new_seeded("migration-skill-locked-file");
        c.ok(&["setup", "claude-code", "--yes"]);
        let before = std::fs::read(skill_path(&c)).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(skill_path(&c))
            .unwrap();
        assert_refused_without_changes(&c, undo);
        drop(lock);
        assert_eq!(std::fs::read(skill_path(&c)).unwrap(), before);
    }
}

#[test]
fn migration_skill_file_ancestor_refuses_apply_and_undo_before_other_writes() {
    for undo in [false, true] {
        let c = Sandbox::new_seeded("migration-skill-file-ancestor");
        if undo {
            c.ok(&["setup", "claude-code", "--yes"]);
            std::fs::remove_file(skill_path(&c)).unwrap();
            std::fs::remove_dir(skill_path(&c).parent().unwrap()).unwrap();
        }
        let ancestor = skill_path(&c).parent().unwrap().to_path_buf();
        std::fs::create_dir_all(ancestor.parent().unwrap()).unwrap();
        std::fs::write(&ancestor, b"synthetic ancestor").unwrap();
        assert_refused_without_changes(&c, undo);
        assert_eq!(std::fs::read(ancestor).unwrap(), b"synthetic ancestor");
    }
}

#[cfg(unix)]
fn link_directory(source: &Path, destination: &Path) {
    std::os::unix::fs::symlink(source, destination).unwrap();
}

#[cfg(windows)]
fn link_directory(source: &Path, destination: &Path) {
    if std::os::windows::fs::symlink_dir(source, destination).is_ok() {
        return;
    }
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(destination)
        .arg(source)
        .output()
        .unwrap();
    assert!(output.status.success(), "could not create test junction");
}

#[test]
fn migration_skill_linked_ancestor_preserves_external_and_project_files() {
    for undo in [false, true] {
        let c = Sandbox::new_seeded("migration-skill-linked-ancestor");
        let external = Sandbox::new_empty("migration-skill-external");
        let external_skill = external.0.join("SKILL.md");
        if undo {
            c.ok(&["setup", "claude-code", "--yes"]);
            std::fs::rename(skill_path(&c), &external_skill).unwrap();
            std::fs::remove_dir(skill_path(&c).parent().unwrap()).unwrap();
        } else {
            std::fs::write(&external_skill, b"synthetic external skill").unwrap();
            std::fs::create_dir_all(skill_path(&c).parent().unwrap().parent().unwrap()).unwrap();
        }
        let before = std::fs::read(&external_skill).unwrap();
        link_directory(&external.0, skill_path(&c).parent().unwrap());
        assert_refused_without_changes(&c, undo);
        assert_eq!(std::fs::read(&external_skill).unwrap(), before);
        // Remove the link before the sandbox's recursive cleanup.
        #[cfg(windows)]
        std::fs::remove_dir(skill_path(&c).parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::fs::remove_file(skill_path(&c).parent().unwrap()).unwrap();
    }
}

#[test]
fn migration_skill_regular_file_keeps_fresh_idempotent_and_edited_undo_behavior() {
    let c = Sandbox::new_seeded("migration-skill-regular");
    c.ok(&["setup", "claude-code", "--yes"]);
    let original = std::fs::read(skill_path(&c)).unwrap();
    c.ok(&["setup", "claude-code", "--yes"]);
    assert_eq!(std::fs::read(skill_path(&c)).unwrap(), original);
    let mut edited = original;
    edited.extend_from_slice(b"\nSynthetic local edit.\n");
    std::fs::write(skill_path(&c), &edited).unwrap();
    c.ok(&["setup", "claude-code", "--undo", "--yes"]);
    assert_eq!(std::fs::read(skill_path(&c)).unwrap(), edited);
}
