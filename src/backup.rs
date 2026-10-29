//! Complete, private local recovery packages.
mod assets;
mod privacy;

use crate::args::Args;
use crate::artifacts::{files, native};
use crate::event::Event;
use crate::failure::Failure;
use crate::model::Tree;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, OpenOptions};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAX_MANIFEST: u64 = 512 * 1024;
const MAX_CONFIG: u64 = 64 * 1024;
const MAX_MARKER: u64 = 8 * 1024;
const MAX_FILES: usize = 4096;
pub(crate) const MAX_FILE: u64 = 256 * 1024 * 1024;
const MAX_TOTAL: u64 = 64 * 1024 * 1024 * 1024;
const BUFFER: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Tree,
    Artifact,
    Agent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    harness: Option<String>,
    path: String,
    size: u64,
    digest: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    identity: String,
    owner_lane: String,
    lanes: Vec<String>,
    files: Vec<Member>,
}

struct Input {
    member: Member,
    file: File,
}

struct LaneInput {
    lane: String,
    path: PathBuf,
    directory: Dir,
    marker: Option<Input>,
    current: bool,
}

struct Snapshot {
    log: Input,
    config: Input,
    marker: Option<Input>,
    lanes: Vec<LaneInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaneMarker {
    version: u32,
    id: String,
    project: String,
}

fn refused(message: &str) -> Failure {
    Failure::usage(format!("Backup refused: {message}"))
}

pub(crate) fn run(cwd: &Path, args: &Args) -> Result<i32, Failure> {
    if args.has("help") {
        if !args.unknown(&["help"]).is_empty()
            || !args.extra(1).is_empty()
            || !args.list("help").is_empty()
        {
            return Err(refused("unexpected arguments with --help."));
        }
        crate::output::outln!("{}", crate::BACKUP_USAGE);
        return Ok(0);
    }
    let command = args.positional(0).unwrap_or_default();
    let allowed: &[&str] = match command {
        "create" => &["lane-root", "json"],
        "verify" => &["json"],
        "recover" => &["to", "source-retired", "json"],
        _ => return Err(refused("expected create, verify or recover.")),
    };
    if !args.unknown(allowed).is_empty() || !args.extra(2).is_empty() {
        return Err(refused("unexpected arguments or options."));
    }
    let path = args
        .positional(1)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| refused("a directory is required."))?;
    let path = absolute(cwd, Path::new(path))?;
    let manifest = match command {
        "create" => create(cwd, args, &path)?,
        "verify" => verify(&path)?,
        "recover" => {
            if !args.has("source-retired") {
                return Err(refused("recovery requires --source-retired."));
            }
            let destinations = args.list("to");
            if destinations.len() != 1 || destinations[0].is_empty() {
                return Err(refused("recovery requires exactly one --to directory."));
            }
            let destination = absolute(cwd, Path::new(&destinations[0]))?;
            recover(&path, &destination)?
        }
        _ => unreachable!(),
    };
    if args.has("json") {
        crate::output::outln!(
            "{}",
            serde_json::json!({"operation":command,"identity":manifest.identity,
                "files":manifest.files.len(),"lanes":manifest.lanes,"verified":true})
        );
    }
    Ok(0)
}

fn absolute(cwd: &Path, path: &Path) -> Result<PathBuf, Failure> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let leaf = path
        .file_name()
        .ok_or_else(|| refused("a named directory is required."))?;
    let parent = path
        .parent()
        .ok_or_else(|| refused("a parent directory is required."))?;
    Ok(parent.canonicalize()?.join(leaf))
}

fn destination(path: &Path) -> Result<(Dir, String), Failure> {
    let parent = path
        .parent()
        .ok_or_else(|| refused("a parent directory is required."))?;
    outside_git(parent)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| refused("the destination name must be UTF-8."))?;
    component(name)?;
    let parent = files::open_root(parent)?;
    match parent.symlink_metadata(name) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
        Ok(_) => return Err(refused("the destination already exists.")),
    }
    Ok((parent, name.to_string()))
}

fn outside_git(parent: &Path) -> Result<(), Failure> {
    for directory in parent.ancestors() {
        match std::fs::symlink_metadata(directory.join(".git")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => {
                return Err(refused(
                    "the destination must be outside Git working trees.",
                ))
            }
        }
    }
    Ok(())
}

fn component(value: &str) -> Result<(), Failure> {
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let device = matches!(
        stem.as_str(),
        "con" | "prn" | "aux" | "nul" | "conin$" | "conout$" | "clock$"
    ) || ["com", "lpt"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|n| {
            matches!(
                n,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    });
    if value.is_empty()
        || matches!(value, "." | "..")
        || value.ends_with(['.', ' '])
        || value.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
        || device
    {
        return Err(refused("invalid relative path component."));
    }
    Ok(())
}

fn relative(path: &str) -> Result<(), Failure> {
    if path.len() > 1024 {
        return Err(refused("a member path exceeds 1,024 bytes."));
    }
    for part in path.split('/') {
        component(part)?;
    }
    Ok(())
}

pub(crate) fn lane_id(lane: &str) -> Result<(), Failure> {
    component(lane)?;
    if lane != crate::lane::MAIN
        && (lane.len() != 26
            || !lane
                .bytes()
                .all(|b| b"0123456789abcdefghjkmnpqrstvwxyz".contains(&b.to_ascii_lowercase())))
    {
        return Err(refused("invalid lane identity."));
    }
    Ok(())
}

fn opened(root: &Dir, path: &str) -> Result<File, Failure> {
    let file = files::regular(root, path)?;
    files::require_single_link(&file)?;
    if file.metadata()?.len() > MAX_FILE {
        return Err(refused("a file exceeds 256 MiB."));
    }
    Ok(file)
}

fn marker(root: &Dir, identity: &str) -> Result<Option<String>, Failure> {
    let mut file = match opened(root, ".vivac/lane") {
        Err(Failure::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        result => result?,
    };
    marker_file(&mut file, identity).map(Some)
}

fn marker_file(file: &mut File, identity: &str) -> Result<String, Failure> {
    file.seek(SeekFrom::Start(0))?;
    let bytes = metadata_bytes(file, MAX_MARKER, "lane marker exceeds 8 KiB.")?;
    privacy::check_json(bytes.as_slice())?;
    let lane: LaneMarker =
        serde_json::from_slice(&bytes).map_err(|_| refused("invalid lane marker."))?;
    lane_id(&lane.id)?;
    if lane.version != 1 || lane.project != identity {
        return Err(refused(
            "the lane marker belongs to a different tree or format.",
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(lane.id)
}

fn captured_marker(input: &mut Option<Input>, identity: &str) -> Result<Option<String>, Failure> {
    input
        .as_mut()
        .map(|input| marker_file(&mut input.file, identity))
        .transpose()
}

fn configuration(file: &mut File) -> Result<(), Failure> {
    file.seek(SeekFrom::Start(0))?;
    let bytes = metadata_bytes(file, MAX_CONFIG, "tree configuration exceeds 64 KiB.")?;
    privacy::check_json(bytes.as_slice())?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| refused("invalid tree configuration."))?;
    let object = value
        .as_object()
        .ok_or_else(|| refused("invalid tree configuration."))?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "version" | "project_id" | "actor" | "share" | "allow_insecure_artifacts"
        )
    }) {
        return Err(refused("tree configuration contains unsupported fields."));
    }
    let _: crate::store::Config =
        serde_json::from_value(value).map_err(|_| refused("invalid tree configuration."))?;
    file.seek(SeekFrom::Start(0))?;
    Ok(())
}

fn metadata_bytes(file: &mut File, limit: u64, message: &str) -> Result<Vec<u8>, Failure> {
    if file.metadata()?.len() > limit {
        return Err(refused(message));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(refused(message));
    }
    Ok(bytes)
}

fn fold(file: &mut File, length: u64) -> Result<(String, Tree), Failure> {
    if length == 0 || length > MAX_FILE {
        return Err(refused("invalid event log size."));
    }
    file.seek(SeekFrom::Start(length - 1))?;
    let mut last = [0];
    file.read_exact(&mut last)?;
    if last[0] != b'\n' {
        return Err(refused("the event log has an incomplete tail."));
    }
    file.seek(SeekFrom::Start(0))?;
    privacy::check_json(BufReader::with_capacity(BUFFER, (&mut *file).take(length)))?;
    file.seek(SeekFrom::Start(0))?;
    let (events, broken): (Vec<Event>, usize) = crate::store::read_all_in(
        &mut BufReader::with_capacity(BUFFER, (&mut *file).take(length)),
    )?;
    if broken != 0 || events.is_empty() {
        return Err(refused("the event log contains unreadable events."));
    }
    let identity = events[0].id.clone();
    let mut seqs = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for event in &events {
        if !seqs.insert(event.seq) || !ids.insert(&event.id) {
            return Err(refused(
                "the event log contains duplicate sequence numbers or identities.",
            ));
        }
        lane_id(&event.lane)?;
    }
    let tree = crate::model::fold(&events, 0);
    if tree.has_pending() || !tree.repeated_nums.is_empty() {
        return Err(refused(
            "the event log contains unresolved or duplicate nodes.",
        ));
    }
    validate_lineage(&tree)?;
    file.seek(SeekFrom::Start(0))?;
    Ok((identity, tree))
}

fn validate_lineage(tree: &Tree) -> Result<(), Failure> {
    let mut validated = BTreeSet::new();
    for node in tree.nodes_iter() {
        if validated.contains(&node.num) {
            continue;
        }
        let mut seen = BTreeSet::new();
        let mut path = Vec::new();
        let mut current = Some(node.num);
        while let Some(num) = current {
            if validated.contains(&num) {
                break;
            }
            if !seen.insert(num) {
                return Err(refused("the event log contains a provenance cycle."));
            }
            path.push(num);
            current = tree
                .node_by_num(num)
                .ok_or_else(|| refused("the event log contains a missing parent."))?
                .parent;
        }
        validated.extend(path);
    }
    Ok(())
}

fn input(root: &Dir, path: &str) -> Result<Input, Failure> {
    let file = opened(root, path)?;
    let size = file.metadata()?.len();
    if (path == ".vivac/config" && size > MAX_CONFIG)
        || (path == ".vivac/lane" && size > MAX_MARKER)
    {
        return Err(refused("tree metadata exceeds its supported size profile."));
    }
    Ok(Input {
        file,
        member: Member {
            kind: Kind::Tree,
            lane: None,
            harness: None,
            path: path.into(),
            size,
            digest: String::new(),
        },
    })
}

fn optional_marker(root: &Dir) -> Result<Option<Input>, Failure> {
    match input(root, ".vivac/lane") {
        Err(Failure::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        result => result.map(Some),
    }
}

fn snapshot(
    root: &Dir,
    root_path: &Path,
    located: &crate::store::Located,
    cwd: &Path,
    args: &Args,
) -> Result<Snapshot, Failure> {
    let (lock_parent, lock_leaf) = files::parent(root, ".vivac/lock")?;
    let mut options = OpenOptions::new();
    options
        .create(true)
        .read(true)
        .write(true)
        .follow(FollowSymlinks::No);
    let lock_file = lock_parent.open_with(lock_leaf, &options)?;
    files::reject_link(&lock_file.metadata()?)?;
    if !lock_file.metadata()?.is_file() {
        return Err(refused("the tree lock must be a regular file."));
    }
    files::require_single_link(&lock_file)?;
    let lock = crate::store::lock_opened_with_deadline(
        lock_file.into_std(),
        &root_path.join(".vivac/lock"),
        std::time::Duration::from_secs(5),
    )?;
    let log = input(root, ".vivac/events")?;
    let config = input(root, ".vivac/config")?;
    let marker = optional_marker(root)?;
    let current = located
        .lane
        .as_ref()
        .map_or(crate::lane::MAIN, |lane| lane.id.as_str());
    let current_directory = files::open_root(&located.lane_dir)?;
    let current_path = located.lane_dir.canonicalize()?;
    let current_marker = optional_marker(&current_directory)?;
    let mut lanes = vec![LaneInput {
        lane: current.into(),
        path: current_path,
        directory: current_directory,
        marker: current_marker,
        current: true,
    }];
    for specification in args.list("lane-root") {
        let (lane, directory) = specification
            .split_once('=')
            .ok_or_else(|| refused("--lane-root requires lane-id=directory."))?;
        if directory.is_empty() {
            return Err(refused("--lane-root requires a nonempty directory."));
        }
        lane_id(lane)?;
        let path = absolute(cwd, Path::new(directory))?;
        let directory = files::open_root(&path)?;
        let path = path.canonicalize()?;
        let marker = optional_marker(&directory)?;
        lanes.push(LaneInput {
            lane: lane.into(),
            path,
            directory,
            marker,
            current: false,
        });
    }
    let snapshot = Snapshot {
        log,
        config,
        marker,
        lanes,
    };
    drop(lock);
    Ok(snapshot)
}

fn create(cwd: &Path, args: &Args, target: &Path) -> Result<Manifest, Failure> {
    if args.has("lane-root") && args.list("lane-root").is_empty() {
        return Err(refused("--lane-root requires a lane-id=directory value."));
    }
    let located = crate::store::locate(cwd)?.ok_or_else(Failure::tree_not_found)?;
    let root = files::open_root(&located.root)?;
    let root_path = located.root.canonicalize()?;
    let Snapshot {
        mut log,
        mut config,
        mut marker,
        lanes,
    } = snapshot(&root, &root_path, &located, cwd, args)?;
    configuration(&mut config.file)?;
    let (identity, tree) = fold(&mut log.file, log.member.size)?;
    let owner =
        captured_marker(&mut marker, &identity)?.unwrap_or_else(|| crate::lane::MAIN.into());
    if marker.is_none() && tree.main_claimed {
        return Err(refused(
            "the owning lane marker is missing after main was claimed.",
        ));
    }
    let mut inputs = vec![log, config];
    if let Some(marker) = marker {
        inputs.push(marker);
    }
    let mut roots = BTreeMap::from([(owner.clone(), root_path.clone())]);
    let mut directories = BTreeMap::from([(owner.clone(), root.try_clone()?)]);
    for mut lane in lanes {
        let actual = captured_marker(&mut lane.marker, &identity)?;
        if actual.as_deref().unwrap_or(crate::lane::MAIN) != lane.lane {
            return Err(refused("a supplied root belongs to a different lane."));
        }
        if roots
            .get(&lane.lane)
            .is_some_and(|previous| previous != &lane.path)
        {
            return Err(refused(if lane.current {
                "the current lane conflicts with the owning root."
            } else {
                "conflicting roots were supplied for a lane."
            }));
        }
        roots.insert(lane.lane.clone(), lane.path);
        directories.insert(lane.lane, lane.directory);
    }
    for (lane, path) in &roots {
        lane_id(lane)?;
        if !tree.lanes.contains_key(lane) {
            return Err(refused("a supplied lane is absent from the log."));
        }
        if target.starts_with(path) || path.starts_with(target) {
            return Err(refused("the backup destination overlaps a source root."));
        }
    }
    for lane in assets::required_lanes(&tree)? {
        if !roots.contains_key(&lane) {
            return Err(refused(
                "a required lane root was not supplied with --lane-root.",
            ));
        }
    }
    let custody_files = assets::collect_with_directories(&tree, &roots, &directories)?;
    drop(tree);
    for asset in custody_files {
        let file = opened(&directories[&asset.lane], &asset.path)?;
        let size = file.metadata()?.len();
        inputs.push(Input {
            file,
            member: Member {
                kind: match asset.kind {
                    assets::AssetKind::Artifact => Kind::Artifact,
                    assets::AssetKind::Agent => Kind::Agent,
                },
                lane: Some(asset.lane),
                harness: asset.harness,
                path: asset.path,
                digest: asset.digest,
                size,
            },
        });
    }
    let mut manifest = Manifest {
        version: 1,
        identity,
        owner_lane: owner,
        lanes: roots.keys().cloned().collect(),
        files: inputs.iter().map(|i| i.member.clone()).collect(),
    };
    // Tree digests are calculated from their pinned descriptors below.
    validate_structure(&manifest, false)?;
    let (parent, name) = destination(target)?;
    let mut preparation = native::Preparation::new(&parent)?;
    write_bytes(&preparation.dir, ".gitignore", b"*\n")?;
    preparation.dir.create_dir("data")?;
    for (ordinal, source) in inputs.iter_mut().enumerate() {
        let mut saved = new_file(&preparation.dir, &payload(ordinal))?;
        let digest = copy(&mut source.file, &mut saved, source.member.size)?;
        if !source.member.digest.is_empty() && source.member.digest != digest {
            return Err(refused("a required custody file changed during backup."));
        }
        manifest.files[ordinal].digest = digest;
        saved.sync_all()?;
    }
    let serialized =
        serde_json::to_vec(&manifest).map_err(|_| refused("cannot encode the manifest."))?;
    if serialized.len() as u64 > MAX_MANIFEST {
        return Err(refused("the manifest exceeds 512 KiB."));
    }
    write_bytes(&preparation.dir, "manifest.json", &serialized)?;
    let validation = native::Preparation::new(&parent)?;
    restore_checked(
        &preparation.dir,
        &manifest,
        &validation.dir,
        &target.parent().unwrap().join(&validation.name),
    )?;
    files::sync_directory(&preparation.dir.open_dir_nofollow("data")?)?;
    native::sync_preparation(&preparation.dir)?;
    crate::registry::backup_origin(&manifest.identity, &root_path)?;
    preparation.publish(&parent, &name)?;
    Ok(manifest)
}

fn payload(ordinal: usize) -> String {
    format!("data/{ordinal:08}")
}

fn new_file(root: &Dir, path: &str) -> Result<File, Failure> {
    let (parent, leaf) = files::parent(root, path)?;
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    Ok(parent.open_with(leaf, &options)?)
}

fn write_bytes(root: &Dir, path: &str, bytes: &[u8]) -> Result<(), Failure> {
    let mut file = new_file(root, path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn copy(reader: &mut impl Read, writer: &mut impl Write, size: u64) -> Result<String, Failure> {
    if size > MAX_FILE {
        return Err(refused("a file exceeds 256 MiB."));
    }
    let mut remaining = size;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; BUFFER];
    while remaining > 0 {
        let count = remaining.min(BUFFER as u64) as usize;
        let count = reader.read(&mut buffer[..count])?;
        if count == 0 {
            return Err(refused("a payload is truncated."));
        }
        writer.write_all(&buffer[..count])?;
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn validate_structure(manifest: &Manifest, digests: bool) -> Result<(), Failure> {
    if manifest.version != 1 || manifest.files.len() > MAX_FILES || manifest.files.len() < 2 {
        return Err(refused("unsupported format or invalid file count."));
    }
    lane_id(&manifest.owner_lane)?;
    if manifest.identity.is_empty() || manifest.identity.len() > 128 {
        return Err(refused("invalid tree identity."));
    }
    let mut lanes = BTreeSet::new();
    let mut lane_aliases = BTreeSet::new();
    for lane in &manifest.lanes {
        lane_id(lane)?;
        if !lane_aliases.insert(lane.to_lowercase()) {
            return Err(refused("duplicate lane identity."));
        }
        lanes.insert(lane.clone());
    }
    if !lanes.contains(&manifest.owner_lane) {
        return Err(refused("the owning lane is missing."));
    }
    let mut total = 0u64;
    let mut paths = BTreeSet::new();
    let mut prefixes = BTreeMap::new();
    let mut tree_paths = BTreeSet::new();
    for file in &manifest.files {
        relative(&file.path)?;
        total = total
            .checked_add(file.size)
            .ok_or_else(|| refused("payload size overflow."))?;
        if file.size > MAX_FILE || total > MAX_TOTAL {
            return Err(refused("payload size limits exceeded."));
        }
        if digests
            && (file.digest.len() != 64
                || !file
                    .digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return Err(refused("invalid SHA-256 digest."));
        }
        match file.kind {
            Kind::Tree => {
                if file.lane.is_some()
                    || file.harness.is_some()
                    || !matches!(
                        file.path.as_str(),
                        ".vivac/events" | ".vivac/config" | ".vivac/lane"
                    )
                {
                    return Err(refused("invalid tree member."));
                }
                if (file.path == ".vivac/config" && file.size > MAX_CONFIG)
                    || (file.path == ".vivac/lane" && file.size > MAX_MARKER)
                {
                    return Err(refused("tree metadata exceeds its supported size profile."));
                }
                tree_paths.insert(file.path.as_str());
            }
            Kind::Artifact | Kind::Agent => {
                let lane = file
                    .lane
                    .as_deref()
                    .ok_or_else(|| refused("a custody member has no lane."))?;
                if !lanes.contains(lane)
                    || (file.kind == Kind::Artifact && file.harness.is_some())
                    || (file.kind == Kind::Agent
                        && file.harness.as_deref().is_none_or(str::is_empty))
                {
                    return Err(refused("invalid custody member metadata."));
                }
                custody_path(file)?;
            }
        }
        let lane = file.lane.as_deref().unwrap_or(&manifest.owner_lane);
        let destination = format!("{lane}/{}", file.path);
        let mut prefix = String::new();
        for part in destination.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if prefixes
                .insert(prefix.to_lowercase(), prefix.clone())
                .is_some_and(|old| old != prefix)
            {
                return Err(refused("case-aliased member directories."));
            }
        }
        if !paths.insert(destination.to_lowercase()) {
            return Err(refused("duplicate or case-aliased member destination."));
        }
    }
    if !tree_paths.contains(".vivac/events") || !tree_paths.contains(".vivac/config") {
        return Err(refused("the tree log or configuration is missing."));
    }
    Ok(())
}

fn custody_path(file: &Member) -> Result<(), Failure> {
    match file.kind {
        Kind::Agent => {
            let adapter = file
                .harness
                .as_deref()
                .and_then(crate::agents::adapters::get)
                .ok_or_else(|| refused("unsupported native agent adapter."))?;
            let suffix = file
                .path
                .strip_prefix(adapter.directory())
                .and_then(|suffix| suffix.strip_prefix('/'))
                .filter(|suffix| !suffix.is_empty())
                .ok_or_else(|| refused("a native agent path leaves its adapter directory."))?;
            if Path::new(suffix)
                .extension()
                .and_then(|value| value.to_str())
                != Some(adapter.extension())
            {
                return Err(refused("a native agent path has an unsupported extension."));
            }
        }
        Kind::Artifact => {
            if file.path == ".vivac-artifacts/.site" {
                return Ok(());
            }
            let object = file
                .path
                .strip_prefix(".vivac-artifacts/objects/")
                .ok_or_else(|| refused("an artifact path leaves retained object storage."))?;
            let (name, child) = object
                .split_once('/')
                .map_or((object, None), |(name, child)| (name, Some(child)));
            let id = if child.is_some() {
                name.strip_suffix(".bundle")
            } else {
                name.strip_suffix(".blob")
            }
            .ok_or_else(|| refused("invalid retained artifact object path."))?;
            if id.len() != 26
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            {
                return Err(refused("invalid retained artifact object identity."));
            }
        }
        Kind::Tree => unreachable!(),
    }
    Ok(())
}

fn read_manifest(root: &Dir) -> Result<Manifest, Failure> {
    let mut file = opened(root, "manifest.json")?;
    if file.metadata()?.len() > MAX_MANIFEST {
        return Err(refused("the manifest exceeds 512 KiB."));
    }
    let mut bytes = Vec::new();
    (&mut file).take(MAX_MANIFEST + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MANIFEST {
        return Err(refused("the manifest exceeds 512 KiB."));
    }
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|_| refused("invalid or unknown manifest format."))?;
    validate_structure(&manifest, true)?;
    let expected = BTreeSet::from([
        "manifest.json".to_string(),
        ".gitignore".into(),
        "data".into(),
    ]);
    if directory_names(root)? != expected {
        return Err(refused("the package has extra or missing members."));
    }
    let mut ignore = opened(root, ".gitignore")?;
    let mut bytes = [0u8; 2];
    if ignore.metadata()?.len() != 2 {
        return Err(refused("invalid package Git exclusion."));
    }
    ignore.read_exact(&mut bytes)?;
    if &bytes != b"*\n" {
        return Err(refused("invalid package Git exclusion."));
    }
    files::reject_link(&root.symlink_metadata("data")?)?;
    let data = root.open_dir_nofollow("data")?;
    let expected: BTreeSet<String> = (0..manifest.files.len())
        .map(|i| format!("{i:08}"))
        .collect();
    if directory_names(&data)? != expected {
        return Err(refused("the payload set is incomplete or has extra files."));
    }
    Ok(manifest)
}

fn directory_names(root: &Dir) -> Result<BTreeSet<String>, Failure> {
    let mut names = BTreeSet::new();
    for entry in root.entries()? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| refused("non-UTF-8 package member."))?;
        if names.len() > MAX_FILES {
            return Err(refused("too many package members."));
        }
        names.insert(name);
    }
    Ok(names)
}

fn ensure_parent(root: &Dir, path: &str) -> Result<(), Failure> {
    let mut current = root.try_clone()?;
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        match current.create_dir(part) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        current = current.open_dir_nofollow(part)?;
    }
    Ok(())
}

fn restored_path(manifest: &Manifest, file: &Member) -> String {
    match file.lane.as_deref() {
        None => file.path.clone(),
        Some(lane) if lane == manifest.owner_lane => file.path.clone(),
        Some(lane) => format!("lanes/{lane}/{}", file.path),
    }
}

fn restore_checked(
    package: &Dir,
    manifest: &Manifest,
    stage: &Dir,
    stage_path: &Path,
) -> Result<(), Failure> {
    validate_structure(manifest, true)?;
    write_bytes(stage, ".gitignore", b"*\n")?;
    let mut restored = BTreeSet::new();
    for (ordinal, member) in manifest.files.iter().enumerate() {
        let mut source = opened(package, &payload(ordinal))?;
        if source.metadata()?.len() != member.size {
            return Err(refused("a payload size does not match its manifest."));
        }
        if member.kind == Kind::Tree {
            privacy::check_json(BufReader::with_capacity(
                BUFFER,
                (&mut source).take(member.size),
            ))?;
            source.seek(SeekFrom::Start(0))?;
        }
        let target = restored_path(manifest, member);
        if !restored.insert(target.to_lowercase()) {
            return Err(refused("colliding recovery paths."));
        }
        ensure_parent(stage, &target)?;
        let mut file = new_file(stage, &target)?;
        let digest = copy(&mut source, &mut file, member.size)?;
        if digest != member.digest || source.metadata()?.len() != member.size {
            return Err(refused(
                "a payload digest or size does not match its manifest.",
            ));
        }
        file.sync_all()?;
    }
    let mut events = opened(stage, ".vivac/events")?;
    let length = events.metadata()?.len();
    let (identity, tree) = fold(&mut events, length)?;
    if identity != manifest.identity {
        return Err(refused("the manifest identity differs from its log."));
    }
    let owner_marker = marker(stage, &identity)?;
    if owner_marker.as_deref().unwrap_or(crate::lane::MAIN) != manifest.owner_lane
        || (owner_marker.is_none() && tree.main_claimed)
    {
        return Err(refused("the owning lane differs from the saved tree."));
    }
    configuration(&mut opened(stage, ".vivac/config")?)?;
    let _store = crate::store::Store::open_strict(stage_path.to_path_buf())?;
    write_bytes(stage, ".vivac/.gitignore", b"*\n")?;
    let mut roots = BTreeMap::new();
    let mut directories = BTreeMap::new();
    for lane in &manifest.lanes {
        if !tree.lanes.contains_key(lane) {
            return Err(refused("an included lane is absent from the log."));
        }
        let path = if lane == &manifest.owner_lane {
            stage_path.to_path_buf()
        } else {
            stage_path.join("lanes").join(lane)
        };
        if lane != &manifest.owner_lane {
            let relative = format!("lanes/{lane}/.vivac/lane");
            ensure_parent(stage, &relative)?;
            let marker = serde_json::to_vec(&crate::lane::Lane {
                version: 1,
                id: lane.clone(),
                project: identity.clone(),
            })
            .map_err(|_| refused("cannot encode lane identity."))?;
            write_bytes(stage, &relative, &marker)?;
            write_bytes(stage, &format!("lanes/{lane}/.vivac/.gitignore"), b"*\n")?;
        }
        let directory = if lane == &manifest.owner_lane {
            stage.try_clone()?
        } else {
            stage.open_dir_nofollow("lanes")?.open_dir_nofollow(lane)?
        };
        directories.insert(lane.clone(), directory);
        roots.insert(lane.clone(), path);
    }
    for lane in assets::required_lanes(&tree)? {
        if !roots.contains_key(&lane) {
            return Err(refused("a required custody lane is absent."));
        }
    }
    let expected = assets::collect_with_directories(&tree, &roots, &directories)?;
    let actual: BTreeSet<_> = manifest
        .files
        .iter()
        .filter(|file| file.kind != Kind::Tree)
        .map(|file| {
            (
                file.lane.clone().unwrap(),
                file.path.clone(),
                file.digest.clone(),
                file.kind == Kind::Agent,
                file.harness.clone(),
            )
        })
        .collect();
    let expected: BTreeSet<_> = expected
        .into_iter()
        .map(|file| {
            (
                file.lane,
                file.path,
                file.digest,
                file.kind == assets::AssetKind::Agent,
                file.harness,
            )
        })
        .collect();
    if expected != actual {
        return Err(refused(
            "the payload set does not match the log's custody references.",
        ));
    }
    sync_children(stage)?;
    native::sync_preparation(stage)?;
    Ok(())
}

fn sync_children(root: &Dir) -> Result<(), Failure> {
    for entry in root.entries()? {
        let entry = entry?;
        let metadata = root.symlink_metadata(entry.file_name())?;
        files::reject_link(&metadata)?;
        if metadata.is_dir() {
            let child = root.open_dir_nofollow(entry.file_name())?;
            sync_children(&child)?;
            files::sync_directory(&child)?;
        }
    }
    Ok(())
}

fn verify(path: &Path) -> Result<Manifest, Failure> {
    let package = files::open_root(path)?;
    let manifest = read_manifest(&package)?;
    let parent_path = path
        .parent()
        .ok_or_else(|| refused("the package has no parent."))?;
    let parent = files::open_root(parent_path)?;
    let preparation = native::Preparation::new(&parent)?;
    restore_checked(
        &package,
        &manifest,
        &preparation.dir,
        &parent_path.join(&preparation.name),
    )?;
    Ok(manifest)
}

fn recover(path: &Path, target: &Path) -> Result<Manifest, Failure> {
    if target.starts_with(path) || path.starts_with(target) {
        return Err(refused("recovery destination overlaps the package."));
    }
    let package = files::open_root(path)?;
    let manifest = read_manifest(&package)?;
    let (parent, name) = destination(target)?;
    let mut preparation = native::Preparation::new(&parent)?;
    restore_checked(
        &package,
        &manifest,
        &preparation.dir,
        &target.parent().unwrap().join(&preparation.name),
    )?;
    let lane_roots = manifest
        .lanes
        .iter()
        .map(|lane| {
            let root = if lane == &manifest.owner_lane {
                target.to_path_buf()
            } else {
                target.join("lanes").join(lane)
            };
            (lane.clone(), root)
        })
        .collect();
    crate::registry::publish_recovery(&manifest.identity, target, &lane_roots, || {
        preparation.publish(&parent, &name)
    })?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(tree: &mut Tree, number: u64, parent: Option<u64>) {
        let body: crate::event::Body = serde_json::from_value(serde_json::json!({
            "type": "node.created", "node": format!("node-{number}"), "num": number,
            "kind": "task", "title": "Synthetic lineage fixture",
            "parent": parent.map(|number| format!("node-{number}"))
        }))
        .unwrap();
        tree.apply(number, "2026-10-29T00:00:00Z", "main", &body);
    }

    fn fixture() -> Manifest {
        Manifest {
            version: 1,
            identity: "01aaaaaaaaaaaaaaaaaaaaaaaa".into(),
            owner_lane: "main".into(),
            lanes: vec!["main".into()],
            files: [".vivac/events", ".vivac/config"]
                .into_iter()
                .map(|path| Member {
                    kind: Kind::Tree,
                    lane: None,
                    harness: None,
                    path: path.into(),
                    size: 0,
                    digest: "0".repeat(64),
                })
                .collect(),
        }
    }

    #[test]
    fn package_bounds_reject_aggregate_sizes_and_integer_overflow() {
        let mut manifest = fixture();
        for ordinal in 0..256 {
            manifest.files.push(Member {
                kind: Kind::Artifact,
                lane: Some("main".into()),
                harness: None,
                path: format!(".vivac-artifacts/objects/{ordinal:026}.blob"),
                size: MAX_FILE,
                digest: "0".repeat(64),
            });
        }
        assert!(
            validate_structure(&manifest, true).is_ok(),
            "exactly 64 GiB is allowed"
        );
        manifest.files[0].size = 1;
        assert!(validate_structure(&manifest, true).is_err());
        manifest.files[0].size = u64::MAX;
        assert!(validate_structure(&manifest, true).is_err());
    }

    #[test]
    fn package_destinations_reject_directory_case_aliases_and_traversal() {
        let mut manifest = fixture();
        for name in ["Review", "review"] {
            manifest.files.push(Member {
                kind: Kind::Agent,
                lane: Some("main".into()),
                harness: Some("codex".into()),
                path: format!(".codex/agents/{name}/{}.toml", manifest.files.len()),
                size: 0,
                digest: "0".repeat(64),
            });
        }
        assert!(validate_structure(&manifest, true).is_err());
        manifest.files.pop();
        manifest.files.last_mut().unwrap().path = ".codex/agents/../../.git/config.toml".into();
        assert!(validate_structure(&manifest, true).is_err());
    }

    #[test]
    fn manifest_fields_and_format_are_closed() {
        let manifest = fixture();
        let mut value = serde_json::to_value(&manifest).unwrap();
        value["origin"] = serde_json::json!("external-machine-path");
        assert!(serde_json::from_value::<Manifest>(value).is_err());
        let mut value = serde_json::to_value(&manifest).unwrap();
        value["files"][0]["payload"] = serde_json::json!("chosen-file");
        assert!(serde_json::from_value::<Manifest>(value).is_err());
        let mut manifest = manifest;
        manifest.version = 2;
        assert!(validate_structure(&manifest, true).is_err());
    }

    #[test]
    fn failed_restoration_installs_git_exclusion_before_reading_payloads() {
        let path = std::env::temp_dir().join(format!("vivac-backup-ignore-{}", crate::id::ulid()));
        std::fs::create_dir(&path).unwrap();
        let parent = files::open_root(&path).unwrap();
        parent.create_dir("package").unwrap();
        let package = parent.open_dir_nofollow("package").unwrap();
        package.create_dir("data").unwrap();
        package.write("data/00000000", b"").unwrap();
        let stage = native::Preparation::new(&parent).unwrap();
        let mut manifest = fixture();
        manifest.files[0].size = 1;
        assert!(restore_checked(&package, &manifest, &stage.dir, &path.join(&stage.name)).is_err());
        assert_eq!(stage.dir.read(".gitignore").unwrap(), b"*\n");
        assert_eq!(
            directory_names(&stage.dir).unwrap(),
            BTreeSet::from([".gitignore".into()])
        );
        drop(stage);
        drop(package);
        drop(parent);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn lineage_validation_accepts_a_deep_chain_without_recursion() {
        let mut tree = Tree::default();
        for number in 1..=20_000 {
            node(&mut tree, number, (number > 1).then_some(number - 1));
        }
        assert_eq!(tree.total(), 20_000);
        assert!(validate_lineage(&tree).is_ok());
    }

    #[test]
    fn lineage_validation_rejects_cycles_and_missing_parents() {
        let mut cycle = Tree::default();
        node(&mut cycle, 1, Some(2));
        node(&mut cycle, 2, Some(1));
        assert!(validate_lineage(&cycle).is_err());
        let mut missing = Tree::default();
        node(&mut missing, 1, Some(2));
        assert!(validate_lineage(&missing).is_err());
    }

    #[test]
    fn metadata_configuration_rejects_valid_json_above_64_kib() {
        let path = std::env::temp_dir().join(format!("vivac-backup-config-{}", crate::id::ulid()));
        std::fs::create_dir(&path).unwrap();
        let root = files::open_root(&path).unwrap();
        let mut bytes = serde_json::to_vec(&serde_json::json!({
            "version": 1, "project_id": "01aaaaaaaaaaaaaaaaaaaaaaaa", "actor": "a_fixture"
        }))
        .unwrap();
        bytes.resize(64 * 1024 + 1, b' ');
        assert!(serde_json::from_slice::<crate::store::Config>(&bytes).is_ok());
        root.write("config", &bytes).unwrap();
        let mut file = opened(&root, "config").unwrap();
        let result = configuration(&mut file);
        drop(file);
        drop(root);
        std::fs::remove_dir_all(path).unwrap();
        assert!(
            result.is_err(),
            "valid JSON above the configuration profile must be refused"
        );
    }

    #[test]
    fn metadata_marker_rejects_valid_json_above_8_kib() {
        let path = std::env::temp_dir().join(format!("vivac-backup-marker-{}", crate::id::ulid()));
        std::fs::create_dir(&path).unwrap();
        let root = files::open_root(&path).unwrap();
        root.create_dir(".vivac").unwrap();
        let identity = "01aaaaaaaaaaaaaaaaaaaaaaaa";
        let mut bytes = serde_json::to_vec(&serde_json::json!({
            "version": 1, "id": "main", "project": identity
        }))
        .unwrap();
        bytes.resize(8 * 1024 + 1, b' ');
        assert!(serde_json::from_slice::<LaneMarker>(&bytes).is_ok());
        root.write(".vivac/lane", &bytes).unwrap();
        let result = marker(&root, identity);
        drop(root);
        std::fs::remove_dir_all(path).unwrap();
        assert!(
            result.is_err(),
            "valid JSON above the marker profile must be refused"
        );
    }

    #[test]
    fn metadata_manifest_rejects_oversized_configuration_before_staging() {
        let mut manifest = fixture();
        manifest.files[1].size = 64 * 1024 + 1;
        assert!(validate_structure(&manifest, true).is_err());
    }

    #[test]
    fn metadata_manifest_rejects_oversized_lane_marker_before_staging() {
        let mut manifest = fixture();
        manifest.files.push(Member {
            kind: Kind::Tree,
            lane: None,
            harness: None,
            path: ".vivac/lane".into(),
            size: 8 * 1024 + 1,
            digest: "0".repeat(64),
        });
        assert!(validate_structure(&manifest, true).is_err());
    }

    fn privacy_event() -> serde_json::Value {
        serde_json::json!({
            "seq": 1, "id": "01aaaaaaaaaaaaaaaaaaaaaaaa", "ts": "2026-10-29T00:00:00Z",
            "actor": "a_fixture", "lane": "main", "payload": {
                "type": "node.created", "node": "01bbbbbbbbbbbbbbbbbbbbbbbb", "num": 1,
                "kind": "task", "title": "Synthetic privacy fixture", "why": "Verify safe recovery"
            }
        })
    }

    fn privacy_fold(bytes: &[u8]) -> Result<(), Failure> {
        let path = std::env::temp_dir().join(format!("vivac-backup-privacy-{}", crate::id::ulid()));
        std::fs::create_dir(&path).unwrap();
        let root = files::open_root(&path).unwrap();
        root.write("events", bytes).unwrap();
        let mut file = opened(&root, "events").unwrap();
        let result = fold(&mut file, bytes.len() as u64).map(|_| ());
        drop(file);
        drop(root);
        std::fs::remove_dir_all(path).unwrap();
        result
    }

    #[test]
    fn privacy_fold_refuses_known_secrets_in_title_and_why() {
        let secret = "ghp_16C7e42F292c6912E7710c838347Ae178B4a";
        assert!(crate::redact::check_field("fixture", secret).is_some());
        for field in ["title", "why"] {
            let mut event = privacy_event();
            event["payload"][field] = serde_json::json!(secret);
            let mut bytes = serde_json::to_vec(&event).unwrap();
            bytes.push(b'\n');
            let result = privacy_fold(&bytes);
            assert!(
                result.is_err(),
                "recovery must not write a known secret in log prose"
            );
            assert!(!result.unwrap_err().message().contains(secret));
        }
    }

    #[test]
    fn privacy_fold_checks_decoded_json_escapes() {
        let secret = "ghp_16C7e42F292c6912E7710c838347Ae178B4a";
        let mut event = privacy_event();
        event["payload"]["title"] = serde_json::json!(secret);
        let mut encoded = serde_json::to_string(&event)
            .unwrap()
            .replace("ghp_", r"\u0067hp_");
        assert!(serde_json::from_str::<Event>(&encoded).is_ok());
        encoded.push('\n');
        let result = privacy_fold(encoded.as_bytes());
        assert!(
            result.is_err(),
            "JSON escaping must not bypass recovery privacy checks"
        );
        assert!(!result.unwrap_err().message().contains(secret));
    }

    #[test]
    fn privacy_fold_checks_raw_fields_and_keys_ignored_by_event_decoding() {
        let secret = "ghp_16C7e42F292c6912E7710c838347Ae178B4a";
        for secret_in_key in [false, true] {
            let mut event = privacy_event();
            if secret_in_key {
                event[secret] = serde_json::json!("Synthetic unknown field");
            } else {
                event["synthetic_unknown_field"] = serde_json::json!(secret);
            }
            let mut bytes = serde_json::to_vec(&event).unwrap();
            assert!(serde_json::from_slice::<Event>(&bytes).is_ok());
            bytes.push(b'\n');
            let result = privacy_fold(&bytes);
            assert!(
                result.is_err(),
                "unknown raw JSON content cannot carry secrets into recovery"
            );
            assert!(!result.unwrap_err().message().contains(secret));
        }
    }

    #[test]
    fn privacy_configuration_refuses_a_personal_email_actor() {
        let path = std::env::temp_dir().join(format!("vivac-backup-actor-{}", crate::id::ulid()));
        std::fs::create_dir(&path).unwrap();
        let root = files::open_root(&path).unwrap();
        let email = "reviewer@example.com";
        assert!(crate::redact::check_field("fixture", email).is_some());
        let bytes = serde_json::to_vec(&serde_json::json!({
            "version": 1, "project_id": "01aaaaaaaaaaaaaaaaaaaaaaaa", "actor": email
        }))
        .unwrap();
        assert!(serde_json::from_slice::<crate::store::Config>(&bytes).is_ok());
        root.write("config", &bytes).unwrap();
        let mut file = opened(&root, "config").unwrap();
        let result = configuration(&mut file);
        drop(file);
        drop(root);
        std::fs::remove_dir_all(path).unwrap();
        assert!(
            result.is_err(),
            "the recovered config actor must remain opaque"
        );
        assert!(!result.unwrap_err().message().contains(email));
    }

    #[test]
    fn snapshot_releases_the_lock_and_preserves_the_captured_prefix() {
        let path = std::env::temp_dir().join(format!("vivac-backup-cut-{}", crate::id::ulid()));
        std::fs::create_dir(&path).unwrap();
        let root = files::open_root(&path).unwrap();
        root.create_dir(".vivac").unwrap();
        let mut prefix = serde_json::to_vec(&privacy_event()).unwrap();
        prefix.push(b'\n');
        root.write(".vivac/events", &prefix).unwrap();
        let config = serde_json::to_vec(&serde_json::json!({
            "version": 1, "project_id": "01aaaaaaaaaaaaaaaaaaaaaaaa", "actor": "a_fixture"
        }))
        .unwrap();
        root.write(".vivac/config", &config).unwrap();
        let identity = "01aaaaaaaaaaaaaaaaaaaaaaaa";
        let lane_bytes = serde_json::to_vec(&serde_json::json!({
            "version": 1, "id": "main", "project": identity
        }))
        .unwrap();
        root.write(".vivac/lane", &lane_bytes).unwrap();
        let located = crate::store::Located {
            root: path.clone(),
            lane_dir: path.clone(),
            lane: Some(crate::lane::Lane {
                version: 1,
                id: "main".into(),
                project: identity.into(),
            }),
            worktree: None,
        };
        let mut cut = snapshot(&root, &path, &located, &path, &Args::default()).unwrap();
        let mut options = OpenOptions::new();
        options.read(true).write(true).follow(FollowSymlinks::No);
        let lock = root.open_with(".vivac/lock", &options).unwrap().into_std();
        assert!(
            lock.try_lock().is_ok(),
            "snapshot must release the writer lock before returning"
        );
        let mut later = privacy_event();
        later["seq"] = serde_json::json!(2);
        later["id"] = serde_json::json!("01cccccccccccccccccccccccc");
        later["payload"]["node"] = serde_json::json!("01dddddddddddddddddddddddd");
        later["payload"]["num"] = serde_json::json!(2);
        later["payload"]["title"] = serde_json::json!("ghp_16C7e42F292c6912E7710c838347Ae178B4a");
        let mut bytes = serde_json::to_vec(&later).unwrap();
        bytes.push(b'\n');
        let mut options = OpenOptions::new();
        options.append(true).follow(FollowSymlinks::No);
        let mut writer = root.open_with(".vivac/events", &options).unwrap();
        writer.write_all(&bytes).unwrap();
        drop(writer);
        lock.unlock().unwrap();
        drop(lock);
        #[cfg(unix)]
        {
            root.remove_file(".vivac/config").unwrap();
            root.write(".vivac/config", b"Changed pathname after the cut")
                .unwrap();
            root.remove_file(".vivac/lane").unwrap();
            root.write(".vivac/lane", b"Changed pathname after the cut")
                .unwrap();
        }
        configuration(&mut cut.config.file).unwrap();
        assert_eq!(
            captured_marker(&mut cut.marker, identity)
                .unwrap()
                .as_deref(),
            Some("main")
        );
        assert_eq!(
            captured_marker(&mut cut.lanes[0].marker, identity)
                .unwrap()
                .as_deref(),
            Some("main")
        );
        let (_, tree) = fold(&mut cut.log.file, cut.log.member.size).unwrap();
        assert_eq!(tree.total(), 1);
        let mut copied = Vec::new();
        copy(&mut cut.log.file, &mut copied, cut.log.member.size).unwrap();
        assert_eq!(copied, prefix);
        let mut copied_config = Vec::new();
        copy(
            &mut cut.config.file,
            &mut copied_config,
            cut.config.member.size,
        )
        .unwrap();
        assert_eq!(copied_config, config);
        let marker = cut.marker.as_mut().unwrap();
        let mut copied_marker = Vec::new();
        copy(&mut marker.file, &mut copied_marker, marker.member.size).unwrap();
        assert_eq!(copied_marker, lane_bytes);
        assert_eq!(
            cut.log.file.metadata().unwrap().len(),
            (prefix.len() + bytes.len()) as u64
        );
        drop(cut);
        drop(root);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn privacy_refuses_unsafe_provenance_before_writing_the_staged_tree() {
        let path =
            std::env::temp_dir().join(format!("vivac-backup-no-write-{}", crate::id::ulid()));
        std::fs::create_dir(&path).unwrap();
        let parent = files::open_root(&path).unwrap();
        parent.create_dir("package").unwrap();
        let package = parent.open_dir_nofollow("package").unwrap();
        package.create_dir("data").unwrap();
        let mut event = privacy_event();
        event["payload"]["why"] = serde_json::json!("ghp_16C7e42F292c6912E7710c838347Ae178B4a");
        let mut bytes = serde_json::to_vec(&event).unwrap();
        bytes.push(b'\n');
        package.write("data/00000000", &bytes).unwrap();
        let mut manifest = fixture();
        manifest.files[0].size = bytes.len() as u64;
        manifest.files[0].digest = format!("{:x}", Sha256::digest(&bytes));
        let stage = native::Preparation::new(&parent).unwrap();
        assert!(restore_checked(&package, &manifest, &stage.dir, &path.join(&stage.name)).is_err());
        assert_eq!(
            directory_names(&stage.dir).unwrap(),
            BTreeSet::from([".gitignore".into()])
        );
        drop(stage);
        drop(package);
        drop(parent);
        std::fs::remove_dir_all(path).unwrap();
    }
}
