//! `map.js` is one function, and inside it `var` belongs to the whole of
//! that function rather than to the block it is written in: two top-level
//! declarations of the same name are one variable, and whichever assignment
//! runs last is what every closure that captured the first one sees.
//!
//! That is how "Where am I?" stopped doing anything. The button and the
//! search's list of hits on the page were both `here`; the page finished
//! loading by assigning the list, so by the time anyone clicked, the
//! button's handler was reading `dataset` off an array and threw before it
//! could move.

use std::collections::BTreeMap;

const MAP_JS: &str = include_str!("../src/web/map.js");

/// Two spaces in is the top level of the one function the file is.
#[test]
fn map_js_declares_no_top_level_name_twice() {
    let mut first_seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut twice = Vec::new();
    for (i, line) in MAP_JS.lines().enumerate() {
        let Some(rest) = line.strip_prefix("  var ") else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        match first_seen.get(&name) {
            Some(first) => twice.push(format!("{name}: lines {first} and {}", i + 1)),
            None => {
                first_seen.insert(name, i + 1);
            }
        }
    }
    assert!(
        first_seen.len() >= 10,
        "only {} top-level declarations found: what broke is the parsing here, not map.js",
        first_seen.len()
    );
    assert!(
        twice.is_empty(),
        "map.js declares a top-level name twice, and the two are one variable:\n  {}",
        twice.join("\n  ")
    );
}
