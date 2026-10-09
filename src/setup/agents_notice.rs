use crate::style::{self, Stream};
use std::path::Path;

pub(super) fn print(root: &Path) {
    let mut counts = Vec::new();
    let mut unreadable = Vec::new();
    for adapter in crate::agents::adapters::all() {
        if !super::doctor::configured(root, adapter.name()) {
            continue;
        }
        match adapter.discover(root) {
            Ok(files) if !files.is_empty() => counts.push((adapter.name().to_owned(), files.len())),
            Ok(_) => {}
            Err(_) => unreadable.push(adapter.name().to_owned()),
        }
    }
    if counts.is_empty() && unreadable.is_empty() {
        return;
    }
    crate::output::outln!("\n  {}", style::bold(Stream::Out, "Custom agents"));
    for (harness, count) in &counts {
        crate::output::outln!("    {harness}: {count} native agent file(s)");
    }
    for harness in unreadable {
        crate::output::outln!(
            "    {}",
            style::warn(
                Stream::Out,
                &format!("{harness}: discovery unavailable; inspect vivac agents.")
            )
        );
    }
    if counts.is_empty() {
        return;
    }
    crate::output::outln!("\n  {}", style::bold(Stream::Out, "With your agent"));
    crate::output::outln!(
        "    Ask: Use the {} skill to select agents and destinations,",
        style::path(Stream::Out, "vivac-agents")
    );
    crate::output::outln!("    then review assignments or synchronize changes.");
    crate::output::outln!("    Open a new harness session to load the installed skill.");
    crate::output::outln!("\n  {}", style::bold(Stream::Out, "In the terminal"));
    crate::output::outln!("    {}", style::path(Stream::Out, "vivac agents sync"));
    crate::output::outln!("\n  {}", style::bold(Stream::Out, "In the web interface"));
    crate::output::outln!("    {}", style::path(Stream::Out, "vivac web"));
    crate::output::outln!(
        "    Open Agents to select and review sources, destinations and assignments."
    );
    crate::output::outln!(
        "\n  {}",
        style::dim(
            Stream::Out,
            "Setup does not transfer agents. Choose the agents you want to manage."
        )
    );
}
