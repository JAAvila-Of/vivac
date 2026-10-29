use crate::artifacts::files;
use crate::event::Repo;
use crate::failure::Failure;
use crate::registry::Sharing;
use cap_fs_ext::DirExt;
use cap_std::fs::{Dir, Metadata};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_REPOSITORIES: usize = 64;
const MAX_SOURCES: usize = 64;
const MAX_CONFIG: u64 = 64 * 1024;
const MAX_LANE: u64 = 8 * 1024;
const MAX_LOG_BYTES: u64 = 64 * 1024 * 1024;
const BUFFER: usize = 64 * 1024;

fn refused() -> Failure {
    Failure::Model(
        "A local repository source could not be checked. Check its tree before retrying, \
         or use --new-tree only for a separate product."
            .into(),
    )
}

pub(super) fn find(here: &Path, repos: &[Repo]) -> Result<Option<Sharing>, Failure> {
    find_checked(here, repos).map_err(|_| refused())
}

fn find_checked(here: &Path, repos: &[Repo]) -> Result<Option<Sharing>, Failure> {
    if repos.len() > MAX_REPOSITORIES {
        return Err(refused());
    }
    let roots: BTreeSet<String> = repos.iter().filter_map(|repo| repo.root.clone()).collect();
    if roots.is_empty() {
        return Ok(None);
    }
    let here = here.canonicalize()?;
    let mut sources = BTreeSet::new();
    for repo in repos {
        if repo.path != "."
            && Path::new(&repo.path)
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(refused());
        }
        let path = here.join(&repo.path);
        if !check_components(&path, false)? {
            return Err(refused());
        }
        let directory = files::open_root(&path)?;
        let Some(config) = configuration(&directory)? else {
            continue;
        };
        for value in origin_values(&config)? {
            let Some(source) = plain_path(&path, &value) else {
                continue;
            };
            if !check_components(&source, true)? {
                continue;
            }
            let source = normalize(&source);
            if !check_components(&source, true)? {
                continue;
            }
            sources.insert(source);
            if sources.len() > MAX_SOURCES {
                return Err(refused());
            }
        }
    }

    let mut inspected = BTreeMap::new();
    let mut matches = BTreeMap::new();
    let mut log_bytes = 0;
    for source in sources {
        let mut candidate = source.as_path();
        for _ in 0..=2 {
            if let Some((root, shared)) =
                tree_at(candidate, &here, &roots, &mut inspected, &mut log_bytes)?
            {
                if !shared.is_empty() {
                    matches.insert(root, shared);
                }
                break;
            }
            let Some(parent) = candidate.parent() else {
                break;
            };
            candidate = parent;
        }
    }
    Ok(matches.into_iter().next().map(|(root, shared)| Sharing {
        name: None,
        root,
        shared,
    }))
}

fn configuration(root: &Dir) -> Result<Option<Vec<u8>>, Failure> {
    let metadata = match root.symlink_metadata(".git") {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    files::reject_link(&metadata)?;
    if metadata.is_file() {
        return Ok(None);
    }
    if !metadata.is_dir() {
        return Err(refused());
    }
    let git = root.open_dir_nofollow(".git")?;
    let mut file = match files::regular(&git, "config") {
        Ok(file) => file,
        Err(Failure::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    files::require_single_link(&file)?;
    Ok(Some(bounded_bytes(&mut file, MAX_CONFIG)?))
}

fn bounded_bytes(file: &mut cap_std::fs::File, limit: u64) -> Result<Vec<u8>, Failure> {
    if file.metadata()?.len() > limit {
        return Err(refused());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(refused());
    }
    Ok(bytes)
}

fn origin_values(config: &[u8]) -> Result<Vec<String>, Failure> {
    if config.len() as u64 > MAX_CONFIG || config.contains(&0) {
        return Err(refused());
    }
    let mut command = Command::new("git");
    command.args([
        "config",
        "--no-includes",
        "--file",
        "-",
        "--null",
        "--get-all",
        "remote.origin.url",
    ]);
    for (key, _) in std::env::vars_os() {
        let upper = key.to_string_lossy().to_ascii_uppercase();
        if upper.starts_with("GIT_CONFIG")
            || matches!(
                upper.as_str(),
                "GIT_DIR" | "GIT_WORK_TREE" | "GIT_COMMON_DIR"
            )
        {
            command.env_remove(key);
        }
    }
    let null_file = if cfg!(windows) { "NUL" } else { "/dev/null" };
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_file)
        .env("GIT_CONFIG_SYSTEM", null_file)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|_| refused())?;
    let Some(mut input) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(refused());
    };
    let Some(output) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(refused());
    };
    let bytes = config.to_vec();
    let writer = std::thread::spawn(move || input.write_all(&bytes));
    let mut bytes = Vec::new();
    let read = output.take(MAX_CONFIG + 1).read_to_end(&mut bytes);
    if read.is_err() || bytes.len() as u64 > MAX_CONFIG {
        let _ = child.kill();
        let _ = child.wait();
        let _ = writer.join();
        return Err(refused());
    }
    let status = child.wait().map_err(|_| refused());
    let written = writer.join().map_err(|_| refused())?;
    written.map_err(|_| refused())?;
    let status = status?;
    if status.code() == Some(1) && bytes.is_empty() {
        return Ok(Vec::new());
    }
    if !status.success() || bytes.last() != Some(&0) {
        return Err(refused());
    }
    let mut values = BTreeSet::new();
    for value in bytes[..bytes.len() - 1].split(|byte| *byte == 0) {
        values.insert(String::from_utf8(value.to_vec()).map_err(|_| refused())?);
        if values.len() > MAX_SOURCES {
            return Err(refused());
        }
    }
    Ok(values.into_iter().collect())
}

fn plain_path(base: &Path, value: &str) -> Option<PathBuf> {
    if value.is_empty()
        || value.starts_with('~')
        || value.starts_with("//")
        || value.starts_with("\\\\")
        || value.contains('\0')
    {
        return None;
    }
    #[cfg(windows)]
    if value.split(['/', '\\']).any(device_component) {
        return None;
    }
    if value.contains(':') {
        #[cfg(windows)]
        {
            let bytes = value.as_bytes();
            if bytes.len() < 3
                || !bytes[0].is_ascii_alphabetic()
                || bytes[1] != b':'
                || !matches!(bytes[2], b'/' | b'\\')
                || value[2..].contains(':')
            {
                return None;
            }
        }
        #[cfg(not(windows))]
        if !Path::new(value).is_absolute() {
            return None;
        }
    }
    let path = Path::new(value);
    if path.has_root() && !path.is_absolute() {
        return None;
    }
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    })
}

#[cfg(windows)]
fn device_component(value: &str) -> bool {
    let stem = value
        .trim_end_matches(['.', ' '])
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_lowercase();
    matches!(
        stem.as_str(),
        "con" | "prn" | "aux" | "nul" | "conin$" | "conout$" | "clock$"
    ) || ["com", "lpt"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|number| {
            matches!(
                number,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    })
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(part.as_os_str()),
        }
    }
    normalized
}

fn check_components(path: &Path, hint: bool) -> Result<bool, Failure> {
    let mut current = PathBuf::new();
    let mut parts = path.components().peekable();
    while let Some(part) = parts.next() {
        current.push(part.as_os_str());
        if matches!(part, Component::Prefix(_)) {
            continue;
        }
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => Metadata::from_just_metadata(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if files::reject_link(&metadata).is_err() {
            return if hint { Ok(false) } else { Err(refused()) };
        }
        if !metadata.is_dir() {
            if hint && metadata.is_file() && parts.peek().is_none() {
                continue;
            }
            return if hint { Ok(false) } else { Err(refused()) };
        }
    }
    Ok(true)
}

fn entry(root: &Dir, path: &str) -> Result<Option<Metadata>, Failure> {
    let metadata = match root.symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    files::reject_link(&metadata)?;
    if !metadata.is_file() {
        return Err(refused());
    }
    Ok(Some(metadata))
}

fn marker_present(root: &Dir, path: &str) -> Result<bool, Failure> {
    match root.symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn tree_at(
    candidate: &Path,
    here: &Path,
    roots: &BTreeSet<String>,
    inspected: &mut BTreeMap<PathBuf, bool>,
    log_bytes: &mut u64,
) -> Result<Option<(PathBuf, Vec<String>)>, Failure> {
    let metadata = match std::fs::symlink_metadata(candidate.join(crate::store::DIR)) {
        Ok(metadata) => Metadata::from_just_metadata(metadata),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(None)
        }
        Err(error) => return Err(error.into()),
    };
    files::reject_link(&metadata)?;
    if !metadata.is_dir() || !check_components(candidate, false)? {
        return Err(refused());
    }
    let candidate = candidate.canonicalize()?;
    if let Some(boundary) = inspected.get(&candidate) {
        return Ok(if *boundary {
            Some((candidate, Vec::new()))
        } else {
            None
        });
    }
    if inspected.len() >= MAX_SOURCES {
        return Err(refused());
    }
    inspected.insert(candidate.clone(), false);
    let directory = files::open_root(&candidate)?;
    let store = directory.open_dir_nofollow(crate::store::DIR)?;
    let Some(config) = crate::store::discovery_config_in(&store)? else {
        let boundary = marker_present(&store, "events")? || marker_present(&store, "lane")?;
        if boundary {
            inspected.insert(candidate.clone(), true);
        }
        return Ok(if boundary {
            Some((candidate, Vec::new()))
        } else {
            None
        });
    };
    inspected.insert(candidate.clone(), true);
    if !config.shares() || crate::anchor::same_folder(&candidate, here) {
        return Ok(Some((candidate, Vec::new())));
    }
    let events = entry(&store, "events")?;
    let lane = entry(&store, "lane")?;
    if events.is_none() {
        return Err(refused());
    }
    let mut file = files::regular(&store, "events")?;
    files::require_single_link(&file)?;
    let length = file.metadata()?.len();
    *log_bytes = log_bytes.checked_add(length).ok_or_else(refused)?;
    if length == 0 || *log_bytes > MAX_LOG_BYTES {
        return Err(refused());
    }
    file.seek(SeekFrom::Start(length - 1))?;
    let mut tail = [0];
    file.read_exact(&mut tail)?;
    if tail != *b"\n" {
        return Err(refused());
    }
    file.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::with_capacity(BUFFER, file.take(length));
    let (identity, source_roots) = crate::store::repository_identity_in(&mut reader)?;
    if reader.into_inner().limit() != 0 {
        return Err(refused());
    }
    if lane.is_some() {
        let mut file = files::regular(&store, "lane")?;
        files::require_single_link(&file)?;
        let bytes = bounded_bytes(&mut file, MAX_LANE)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| refused())?;
        let object = value.as_object().ok_or_else(refused)?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "version" | "id" | "project"))
        {
            return Err(refused());
        }
        let lane: crate::lane::Lane = serde_json::from_value(value).map_err(|_| refused())?;
        if lane.version != 1 || lane.project != identity || lane.id.is_empty() {
            return Err(refused());
        }
    }
    let shared = source_roots
        .into_iter()
        .filter(|root| roots.contains(root))
        .collect();
    Ok(Some((candidate, shared)))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        here: PathBuf,
        source: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("vivac-local-sources-{}", crate::id::ulid()));
            std::fs::create_dir(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let here = root.join("current");
            let source = root.join("source");
            std::fs::create_dir_all(here.join(".git")).unwrap();
            std::fs::create_dir_all(source.join("repo")).unwrap();
            std::fs::create_dir_all(source.join(".vivac")).unwrap();
            let fixture = Self { root, here, source };
            fixture.set_origin(&fixture.source.join("repo"));
            std::fs::write(fixture.source.join(".vivac/config"), fixture.config(true)).unwrap();
            std::fs::write(fixture.source.join(".vivac/events"), fixture.events()).unwrap();
            fixture
        }

        fn path_text(origin: &Path) -> String {
            #[cfg(windows)]
            let verbatim_disk = matches!(
                origin.components().next(),
                Some(Component::Prefix(prefix)) if matches!(prefix.kind(), std::path::Prefix::VerbatimDisk(_))
            );
            let origin = origin.to_str().unwrap().replace('\\', "/");
            #[cfg(windows)]
            let origin = if verbatim_disk {
                origin[4..].to_string()
            } else {
                origin
            };
            origin
        }

        fn set_origin(&self, origin: &Path) {
            let quoted = Self::path_text(origin).replace('"', "\\\"");
            std::fs::write(
                self.here.join(".git/config"),
                format!("[remote \"origin\"]\nurl = \"{quoted}\"\n"),
            )
            .unwrap();
        }

        fn repos(&self) -> Vec<Repo> {
            vec![Repo {
                path: ".".into(),
                root: Some("a".repeat(40)),
            }]
        }

        fn config(&self, share: bool) -> Vec<u8> {
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "project_id": "00000000000000000000000001",
                "actor": "opaque",
                "share": share
            }))
            .unwrap()
        }

        fn events(&self) -> Vec<u8> {
            let event: crate::event::Event = crate::event::Event {
                seq: 1,
                id: "00000000000000000000000001".into(),
                ts: "2026-01-01T00:00:00Z".into(),
                actor: "opaque".into(),
                lane: "main".into(),
                payload: crate::event::Body::LaneDeclared {
                    lane: "main".into(),
                    name: "Synthetic lane".into(),
                    repos: self.repos(),
                },
            };
            let mut bytes = serde_json::to_vec(&event).unwrap();
            bytes.push(b'\n');
            bytes
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn local_sources_find_matching_metadata_without_writing_source_files_or_indexes() {
        let fixture = Fixture::new();
        let events = std::fs::read(fixture.source.join(".vivac/events")).unwrap();
        let config = std::fs::read(fixture.source.join(".vivac/config")).unwrap();
        let sharing = find(&fixture.here, &fixture.repos()).unwrap().unwrap();
        assert_eq!(sharing.root, fixture.source);
        assert_eq!(sharing.shared, vec!["a".repeat(40)]);
        assert!(sharing.name.is_none());
        assert_eq!(
            std::fs::read(fixture.source.join(".vivac/events")).unwrap(),
            events
        );
        assert_eq!(
            std::fs::read(fixture.source.join(".vivac/config")).unwrap(),
            config
        );
        let mut names: Vec<_> = std::fs::read_dir(fixture.source.join(".vivac"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, vec!["config", "events"]);
    }

    #[test]
    fn local_sources_ignore_unrelated_roots_and_do_not_follow_another_origin() {
        let fixture = Fixture::new();
        let repos = [Repo {
            path: ".".into(),
            root: Some("b".repeat(40)),
        }];
        assert!(find(&fixture.here, &repos).unwrap().is_none());
        let intermediate = fixture.root.join("intermediate/repo");
        std::fs::create_dir_all(intermediate.join(".git")).unwrap();
        std::fs::copy(
            fixture.here.join(".git/config"),
            intermediate.join(".git/config"),
        )
        .unwrap();
        fixture.set_origin(&intermediate);
        assert!(find(&fixture.here, &fixture.repos()).unwrap().is_none());
    }

    #[test]
    fn local_sources_keep_the_same_two_parent_limit_for_missing_and_file_origins() {
        let fixture = Fixture::new();
        for leaf in [
            fixture.source.join("missing"),
            fixture.source.join("bundle"),
        ] {
            if leaf.ends_with("bundle") {
                std::fs::write(&leaf, b"not source metadata").unwrap();
            }
            fixture.set_origin(&leaf);
            assert_eq!(
                find(&fixture.here, &fixture.repos()).unwrap().unwrap().root,
                fixture.source
            );
        }
        let deep = fixture.source.join("one/two/missing");
        std::fs::create_dir_all(deep.parent().unwrap()).unwrap();
        fixture.set_origin(&deep);
        assert!(find(&fixture.here, &fixture.repos()).unwrap().is_none());
    }

    #[test]
    fn local_sources_use_git_quoting_ignore_includes_and_keep_every_origin() {
        let fixture = Fixture::new();
        let include = fixture.root.join("included");
        std::fs::write(&include, "[remote \"origin\"]\nurl=outside\n").unwrap();
        let include = Fixture::path_text(&include);
        let config = format!(
            "[include]\npath=\"{include}\"\n[remote \"origin\"]\nurl=\"a\\\\b\\\"c\"\nurl=second\nurl=second\n"
        );
        assert_eq!(
            origin_values(config.as_bytes()).unwrap(),
            vec!["a\\b\"c", "second"]
        );
        assert!(origin_values(b"[core]\nbare=false\n").unwrap().is_empty());
        assert!(origin_values(b"[malformed\nprivate text").is_err());
    }

    #[test]
    fn local_sources_reject_nul_config_instead_of_accepting_a_truncated_origin() {
        assert!(origin_values(b"[remote \"origin\"]\nurl=/synthetic/a\0/synthetic/b\n").is_err());
    }

    #[test]
    fn local_sources_ignore_network_helpers_devices_and_tilde_without_path_access() {
        for value in [
            "https://example.invalid/project",
            "ssh://example.invalid/project",
            "git@example.invalid:project",
            "ext::sh -c synthetic",
            "file:///tmp/project",
            "//server/share/project",
            "\\\\server\\share\\project",
            "\\\\.\\device",
            "\\\\?\\C:\\project",
            "~/project",
            "~role/project",
            "",
        ] {
            assert!(
                plain_path(Path::new("base"), value).is_none(),
                "unsupported hint was accepted"
            );
        }
        assert_eq!(
            plain_path(Path::new("base"), "../source"),
            Some(PathBuf::from("base/../source"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_sources_accept_colons_in_absolute_unix_paths() {
        assert_eq!(
            plain_path(Path::new("/synthetic/base"), "/synthetic/source:clone"),
            Some(PathBuf::from("/synthetic/source:clone"))
        );
        assert!(plain_path(Path::new("/synthetic/base"), "host:source").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn local_sources_reject_dos_device_components_before_path_access() {
        for value in [
            "NUL",
            "con.txt",
            "PRN.",
            "AUX ",
            "CONIN$",
            "CONOUT$.txt",
            "C:/local/NUL.txt",
            "C:/local/COM1",
            "C:/local/LPT9.log",
            "C:/local/COM¹.log",
            "C:/local/LPT²",
            "C:/local/COM³ ",
        ] {
            assert!(
                plain_path(Path::new("base"), value).is_none(),
                "device component was accepted"
            );
        }
        for value in ["COM10", "normal/console", "normal/NULfile"] {
            assert!(plain_path(Path::new("base"), value).is_some());
        }
    }

    #[test]
    fn local_sources_refuse_config_repository_and_origin_limits_without_echoing_values() {
        let fixture = Fixture::new();
        let configs = [
            vec![b'#'; MAX_CONFIG as usize + 1],
            format!(
                "[remote \"origin\"]\n{}",
                (0..65)
                    .map(|index| format!("url=source-{index}\n"))
                    .collect::<String>()
            )
            .into_bytes(),
        ];
        for config in configs {
            std::fs::write(fixture.here.join(".git/config"), &config).unwrap();
            let error = find(&fixture.here, &fixture.repos()).unwrap_err();
            assert_eq!(error.message(), refused().message());
        }
        let repos = vec![fixture.repos()[0].clone(); 65];
        assert_eq!(
            find(&fixture.here, &repos).unwrap_err().message(),
            refused().message()
        );
    }

    #[test]
    fn local_sources_refuse_torn_unknown_empty_and_oversized_authorities() {
        let fixture = Fixture::new();
        for events in [
            Vec::new(),
            fixture.events()[..fixture.events().len() - 1].to_vec(),
            b"{\"type\":\"future.synthetic\",\"private\":\"withheld text\"}\n".to_vec(),
        ] {
            std::fs::write(fixture.source.join(".vivac/events"), events).unwrap();
            let error = find(&fixture.here, &fixture.repos()).unwrap_err();
            assert_eq!(error.message(), refused().message());
            assert!(!error.message().contains("withheld"));
        }
        let file = std::fs::File::create(fixture.source.join(".vivac/events")).unwrap();
        file.set_len(MAX_LOG_BYTES + 1).unwrap();
        assert!(find(&fixture.here, &fixture.repos()).is_err());
        drop(file);
        std::fs::write(fixture.source.join(".vivac/events"), fixture.events()).unwrap();
        let mut bytes = MAX_LOG_BYTES - fixture.events().len() as u64 + 1;
        assert!(tree_at(
            &fixture.source,
            &fixture.here,
            &BTreeSet::new(),
            &mut BTreeMap::new(),
            &mut bytes
        )
        .is_err());
    }

    #[test]
    fn local_sources_validate_tree_lane_markers_and_skip_sources_with_no_config() {
        let fixture = Fixture::new();
        for lane in [
            serde_json::json!({"version":2,"id":"main","project":"00000000000000000000000001"}),
            serde_json::json!({"version":1,"id":"main","project":"different"}),
            serde_json::json!({"version":1,"id":"main","project":"00000000000000000000000001","extra":"withheld text"}),
        ] {
            std::fs::write(
                fixture.source.join(".vivac/lane"),
                serde_json::to_vec(&lane).unwrap(),
            )
            .unwrap();
            assert_eq!(
                find(&fixture.here, &fixture.repos()).unwrap_err().message(),
                refused().message()
            );
        }
        let valid =
            serde_json::json!({"version":1,"id":"main","project":"00000000000000000000000001"});
        std::fs::write(
            fixture.source.join(".vivac/lane"),
            serde_json::to_vec(&valid).unwrap(),
        )
        .unwrap();
        assert!(find(&fixture.here, &fixture.repos()).unwrap().is_some());
        std::fs::remove_file(fixture.source.join(".vivac/events")).unwrap();
        std::fs::remove_file(fixture.source.join(".vivac/config")).unwrap();
        assert!(find(&fixture.here, &fixture.repos()).unwrap().is_none());
    }

    #[test]
    fn local_sources_skip_closed_trees_before_reading_invalid_events() {
        let fixture = Fixture::new();
        std::fs::write(fixture.source.join(".vivac/config"), fixture.config(false)).unwrap();
        let events = b"private event text is not valid JSON\n";
        std::fs::write(fixture.source.join(".vivac/events"), events).unwrap();
        assert!(find(&fixture.here, &fixture.repos()).unwrap().is_none());
        assert_eq!(
            std::fs::read(fixture.source.join(".vivac/events")).unwrap(),
            events
        );
    }

    #[test]
    fn local_sources_follow_sharing_after_config_regeneration_without_changing_tree_identity() {
        let fixture = Fixture::new();
        let events = std::fs::read(fixture.source.join(".vivac/events")).unwrap();
        let first: serde_json::Value =
            serde_json::from_slice(events.split(|byte| *byte == b'\n').next().unwrap()).unwrap();
        let lane = serde_json::to_vec(&serde_json::json!({
            "version": 1, "id": "main", "project": first["id"]
        }))
        .unwrap();
        std::fs::write(fixture.source.join(".vivac/lane"), &lane).unwrap();
        let mut config: serde_json::Value = serde_json::from_slice(&fixture.config(false)).unwrap();
        config["project_id"] = serde_json::json!("00000000000000000000000002");
        assert_ne!(config["project_id"], first["id"]);
        let closed = serde_json::to_vec(&config).unwrap();
        std::fs::write(fixture.source.join(".vivac/config"), &closed).unwrap();

        assert!(find(&fixture.here, &fixture.repos()).unwrap().is_none());
        assert_eq!(
            std::fs::read(fixture.source.join(".vivac/config")).unwrap(),
            closed
        );
        assert_eq!(
            std::fs::read(fixture.source.join(".vivac/events")).unwrap(),
            events
        );
        assert_eq!(
            std::fs::read(fixture.source.join(".vivac/lane")).unwrap(),
            lane
        );

        config["share"] = serde_json::json!(true);
        let open = serde_json::to_vec(&config).unwrap();
        std::fs::write(fixture.source.join(".vivac/config"), &open).unwrap();
        let sharing = find(&fixture.here, &fixture.repos()).unwrap().unwrap();
        assert_eq!(sharing.root, fixture.source);
        assert_eq!(sharing.shared, vec!["a".repeat(40)]);
        assert_eq!(
            std::fs::read(fixture.source.join(".vivac/config")).unwrap(),
            open
        );
        assert_eq!(
            std::fs::read(fixture.source.join(".vivac/events")).unwrap(),
            events
        );
        assert_eq!(
            std::fs::read(fixture.source.join(".vivac/lane")).unwrap(),
            lane
        );
    }

    #[test]
    fn local_sources_skip_missing_config_before_reading_invalid_events() {
        let fixture = Fixture::new();
        std::fs::remove_file(fixture.source.join(".vivac/config")).unwrap();
        let events = b"event text with no sharing policy\n";
        std::fs::write(fixture.source.join(".vivac/events"), events).unwrap();
        assert!(find(&fixture.here, &fixture.repos()).unwrap().is_none());
        assert!(!fixture.source.join(".vivac/config").exists());
        assert_eq!(
            std::fs::read(fixture.source.join(".vivac/events")).unwrap(),
            events
        );
    }

    #[test]
    fn local_sources_refuse_invalid_or_future_configs_without_echoing_them() {
        let fixture = Fixture::new();
        for config in [
            b"not a config with withheld text".to_vec(),
            serde_json::to_vec(&serde_json::json!({
                "version": 2, "project_id": "opaque", "actor": "opaque"
            }))
            .unwrap(),
        ] {
            std::fs::write(fixture.source.join(".vivac/config"), config).unwrap();
            let error = find(&fixture.here, &fixture.repos()).unwrap_err();
            assert_eq!(error.message(), refused().message());
            assert!(!error.message().contains("withheld"));
        }
    }

    #[test]
    fn local_sources_do_not_read_above_a_closed_or_unrelated_tree_boundary() {
        for closed in [false, true] {
            let mut fixture = Fixture::new();
            let parent = fixture.root.join("parent");
            std::fs::create_dir(&parent).unwrap();
            let child = parent.join("child");
            std::fs::rename(&fixture.source, &child).unwrap();
            fixture.source = child;
            fixture.set_origin(&fixture.source.join("repo"));
            std::fs::write(
                fixture.source.join(".vivac/config"),
                fixture.config(!closed),
            )
            .unwrap();
            let unrelated = String::from_utf8(fixture.events())
                .unwrap()
                .replace(&"a".repeat(40), &"b".repeat(40));
            std::fs::write(fixture.source.join(".vivac/events"), unrelated).unwrap();
            std::fs::create_dir(parent.join(".vivac")).unwrap();
            std::fs::write(parent.join(".vivac/config"), fixture.config(true)).unwrap();
            std::fs::write(parent.join(".vivac/events"), fixture.events()).unwrap();
            assert!(find(&fixture.here, &fixture.repos()).unwrap().is_none());
        }
    }

    #[test]
    fn local_sources_keep_repeated_tree_boundaries_and_check_every_origin() {
        let mut fixture = Fixture::new();
        let config = std::fs::read(fixture.here.join(".git/config")).unwrap();
        let second = fixture.root.join("z-last");
        std::fs::create_dir_all(second.join("repo")).unwrap();
        std::fs::create_dir(second.join(".vivac")).unwrap();
        std::fs::write(second.join(".vivac/config"), fixture.config(true)).unwrap();
        std::fs::write(second.join(".vivac/events"), fixture.events()).unwrap();
        fixture.set_origin(&second.join("repo"));
        let mut both = config.clone();
        both.extend(std::fs::read(fixture.here.join(".git/config")).unwrap());
        std::fs::write(fixture.here.join(".git/config"), &both).unwrap();
        assert_eq!(
            find(&fixture.here, &fixture.repos()).unwrap().unwrap().root,
            fixture.source
        );
        std::fs::write(second.join(".vivac/config"), b"invalid final source policy").unwrap();
        assert!(find(&fixture.here, &fixture.repos()).is_err());

        let parent = fixture.root.join("separate");
        std::fs::create_dir(&parent).unwrap();
        let moved = parent.join("child");
        std::fs::rename(&fixture.source, &moved).unwrap();
        fixture.source = moved;
        std::fs::create_dir(parent.join(".vivac")).unwrap();
        std::fs::write(parent.join(".vivac/config"), fixture.config(true)).unwrap();
        std::fs::write(parent.join(".vivac/events"), fixture.events()).unwrap();
        let unrelated = String::from_utf8(fixture.events())
            .unwrap()
            .replace(&"a".repeat(40), &"b".repeat(40));
        std::fs::write(fixture.source.join(".vivac/events"), unrelated).unwrap();
        std::fs::create_dir(fixture.source.join("other")).unwrap();
        fixture.set_origin(&fixture.source.join("repo"));
        let mut repeated = std::fs::read(fixture.here.join(".git/config")).unwrap();
        fixture.set_origin(&fixture.source.join("other"));
        repeated.extend(std::fs::read(fixture.here.join(".git/config")).unwrap());
        std::fs::write(fixture.here.join(".git/config"), repeated).unwrap();
        assert!(find(&fixture.here, &fixture.repos()).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn local_sources_skip_alias_hints_but_refuse_a_linked_positive_marker() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let alias = fixture.root.join("alias");
        symlink(&fixture.source, &alias).unwrap();
        fixture.set_origin(&alias.join("repo"));
        assert!(find(&fixture.here, &fixture.repos()).unwrap().is_none());
        fixture.set_origin(&fixture.source.join("repo"));
        let store = fixture.source.join(".vivac");
        let moved = fixture.root.join("retained");
        std::fs::rename(&store, &moved).unwrap();
        symlink(&moved, &store).unwrap();
        assert!(find(&fixture.here, &fixture.repos()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn local_sources_refuse_special_files_without_blocking() {
        const MODE: &str = "VIVAC_TEST_LOCAL_SOURCES_SPECIAL_FILE";
        if let Ok(mode) = std::env::var(MODE) {
            let fixture = Fixture::new();
            let path = if mode == "config" {
                fixture.here.join(".git/config")
            } else {
                fixture.source.join(".vivac/events")
            };
            std::fs::remove_file(&path).unwrap();
            assert!(Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success());
            assert!(find(&fixture.here, &fixture.repos()).is_err());
            return;
        }
        for mode in ["config", "events"] {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "setup::local_sources::tests::local_sources_refuse_special_files_without_blocking"])
                .env(MODE, mode).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("source metadata inspection blocked on a special file");
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
}
