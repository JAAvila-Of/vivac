mod common;
use common::Sandbox;

#[test]
fn bare_agents_lists_native_agents_without_importing_or_terminal_controls() {
    let c = Sandbox::new_seeded("agents-human-inventory");
    std::fs::create_dir_all(c.0.join(".claude/agents")).unwrap();
    std::fs::write(c.0.join(".claude/agents/reviewer.md"),
        "---\nname: reviewer\ndescription: Review changes.\nmodel: sonnet\n---\nReview the change.\n").unwrap();
    let before = c.log();
    let text = c.ok(&["agents"]);
    assert!(text.contains("reviewer"), "{text}");
    assert!(text.contains("claude-code"), "{text}");
    assert!(text.contains("not managed"), "{text}");
    assert!(!text.contains('\u{1b}'), "{text}");
    assert_eq!(before, c.log());
}
