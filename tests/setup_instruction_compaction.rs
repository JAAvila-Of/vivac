mod common;

use common::Sandbox;
use std::path::{Path, PathBuf};
use std::process::Command;

const SKILLS: [(&str, &str); 3] = [
    (
        "vivac-migrate",
        concat!(
            include_str!("data/instruction-legacy/skill-frontmatter.md"),
            include_str!("data/instruction-legacy/skill-body.md")
        ),
    ),
    (
        "vivac-agents",
        include_str!("data/instruction-legacy/agents-skill.md"),
    ),
    (
        "vivac-artifacts",
        include_str!("data/instruction-legacy/artifacts-skill.md"),
    ),
];

fn marked(content: &str) -> String {
    let end = content
        .strip_prefix("---\n")
        .unwrap()
        .find("\n---\n")
        .unwrap()
        + 9;
    let (frontmatter, body) = content.split_at(end);
    let hash = content.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{frontmatter}<!-- written by vivac setup; fingerprint {hash:016x}; setup removes it with --undo while the text is unchanged -->\n{body}")
}

fn setup(c: &Sandbox, harness: &str) -> (String, i32) {
    let output = Command::new(env!("CARGO_BIN_EXE_vivac"))
        .current_dir(&c.0)
        .env("VIVAC_HOME", c.global_home())
        .env("CODEX_HOME", c.0.join("isolated-codex-home"))
        .env("NO_COLOR", "1")
        .args(["setup", harness, "--yes"])
        .output()
        .unwrap();
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        output.status.code().unwrap_or(-1),
    )
}

fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, dir: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
        if !dir.exists() {
            return;
        }
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                ));
            }
        }
    }
    let mut files = Vec::new();
    visit(root, root, &mut files);
    files.sort();
    files
}

#[test]
fn original_release_skills_upgrade_together_without_touching_the_tree() {
    for (harness, folder) in [("codex", ".agents"), ("claude-code", ".claude")] {
        let c = Sandbox::new_seeded("instruction-upgrade");
        let (out, code) = setup(&c, harness);
        assert_eq!(code, 0, "{out}");
        let paths: Vec<_> = SKILLS
            .iter()
            .map(|(name, _)| c.0.join(folder).join("skills").join(name).join("SKILL.md"))
            .collect();
        let expected: Vec<_> = paths
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect();
        for ((_, content), path) in SKILLS.iter().zip(&paths) {
            let old = marked(content);
            assert_ne!(std::fs::read(path).unwrap(), old.as_bytes());
            std::fs::write(path, old).unwrap();
        }
        let tree = snapshot(&c.0.join(".vivac"));
        let (out, code) = setup(&c, harness);
        assert_eq!(code, 0, "{out}");
        for (path, expected) in paths.iter().zip(&expected) {
            assert_eq!(std::fs::read(path).unwrap(), *expected, "{path:?}");
        }
        assert_eq!(snapshot(&c.0.join(".vivac")), tree);
    }
}

#[test]
fn editing_any_original_release_skill_blocks_the_entire_upgrade() {
    for (harness, folder) in [("codex", ".agents"), ("claude-code", ".claude")] {
        for (edited, &(edited_name, _)) in SKILLS.iter().enumerate() {
            let c = Sandbox::new_seeded("instruction-upgrade-conflict");
            let (out, code) = setup(&c, harness);
            assert_eq!(code, 0, "{out}");
            for (index, (name, content)) in SKILLS.iter().enumerate() {
                let path = c.0.join(folder).join("skills").join(name).join("SKILL.md");
                let mut old = marked(content);
                if index == edited {
                    old.push_str("\nSynthetic local instruction edit.\n");
                }
                std::fs::write(path, old).unwrap();
            }
            let before = snapshot(&c.0);
            let registry = snapshot(c.global_home());
            let (out, code) = setup(&c, harness);
            assert_eq!(code, 1, "{out}");
            assert!(out.contains(edited_name), "{out}");
            assert!(out.contains("Nothing written"), "{out}");
            assert_eq!(snapshot(&c.0), before);
            assert_eq!(snapshot(c.global_home()), registry);
        }
    }
}
