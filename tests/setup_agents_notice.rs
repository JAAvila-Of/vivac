mod common;
use common::Sandbox;
use std::process::Command;

fn agent(c: &Sandbox) {
    let path = c.0.join(".claude/agents/reviewer.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "---\nname: reviewer\ndescription: Review changes.\nmodel: sonnet\n---\nPRIVATE_PROMPT_BODY\n").unwrap();
}

#[test]
fn invitation_requires_custom_agents_in_a_configured_harness() {
    let c = Sandbox::new_seeded("agents-notice");
    agent(&c);
    let out = c.ok(&["setup", "codex", "--yes"]);
    assert!(!out.contains("Custom agents"), "{out}");
    let out = c.ok(&["setup", "claude-code", "--yes"]);
    assert!(out.contains("claude-code: 1 native agent file(s)"), "{out}");
    let out = c.ok(&["setup", "codex", "--yes"]);
    for part in [
        "Custom agents",
        "With your agent",
        "vivac-agents",
        "In the terminal",
        "vivac agents sync",
        "In the web interface",
        "vivac web",
        "Setup does not transfer agents",
    ] {
        assert!(out.contains(part), "Missing {part}: {out}");
    }
    assert!(!out.contains("PRIVATE_PROMPT_BODY"));
    for args in [
        vec!["setup", "codex", "--dry-run"],
        vec!["setup", "codex", "--undo", "--dry-run"],
        vec!["setup", "codex", "--undo", "--yes"],
    ] {
        let out = c.ok(&args);
        assert!(!out.contains("Custom agents"), "{out}");
    }
}

#[test]
fn empty_projects_install_the_skill_without_an_invitation() {
    let c = Sandbox::new_seeded("agents-empty-notice");
    for (harness, path) in [
        ("claude-code", ".claude/skills/vivac-agents/SKILL.md"),
        ("codex", ".agents/skills/vivac-agents/SKILL.md"),
    ] {
        let out = c.ok(&["setup", harness, "--yes"]);
        assert!(c.0.join(path).is_file());
        assert!(!out.contains("Custom agents"), "{out}");
    }
}

#[test]
fn discovery_errors_are_not_reported_as_empty_inventories() {
    let c = Sandbox::new_seeded("agents-notice-error");
    c.ok(&["setup", "claude-code", "--yes"]);
    let path = c.0.join(".claude/agents");
    std::fs::write(path, "not a directory").unwrap();
    let out = c.ok(&["setup", "codex", "--yes"]);
    assert!(out.contains("claude-code: discovery unavailable"), "{out}");
    assert!(!out.contains("0 native agent"));
    assert!(!out.contains("With your agent"));
}

#[test]
fn color_changes_presentation_without_changing_the_notice() {
    let c = Sandbox::new_seeded("agents-notice-color");
    agent(&c);
    c.ok(&["setup", "claude-code", "--yes"]);
    let run = |plain: Option<(&str, &str)>| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_vivac"));
        cmd.current_dir(&c.0)
            .env("VIVAC_HOME", c.global_home())
            .env_remove("NO_COLOR")
            .env_remove("TERM")
            .env("CLICOLOR_FORCE", "1")
            .args(["setup", "claude-code", "--yes"]);
        if let Some((key, value)) = plain {
            cmd.env(key, value);
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = String::from_utf8(out.stdout).unwrap();
        text[text.find("\n  ").unwrap_or(0)..].to_owned()
    };
    let styled = run(None);
    assert!(styled.contains("\x1b[1mCustom agents"));
    assert!(styled.contains("\x1b[36mvivac agents sync"));
    let mut clean = String::new();
    let mut chars = styled.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            for ch in chars.by_ref() {
                if ch == 'm' {
                    break;
                }
            }
        } else {
            clean.push(ch);
        }
    }
    for mode in [("NO_COLOR", "1"), ("TERM", "dumb")] {
        let plain = run(Some(mode));
        assert!(!plain.contains('\x1b'));
        assert_eq!(plain, clean);
    }
}
