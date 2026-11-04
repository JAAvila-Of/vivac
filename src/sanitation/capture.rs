use super::*;
use crate::artifacts::files;
use crate::backup;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

#[cfg(windows)]
fn identity(handle: &impl std::os::windows::io::AsRawHandle) -> Result<Identity> {
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
    };
    let mut info = FILE_ID_INFO::default();
    // SAFETY: the borrowed capability owns a live handle for the entire call;
    // the maintained binding supplies the correctly sized output structure.
    if unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(refused());
    }
    Ok(Identity {
        volume: info.VolumeSerialNumber,
        file: info.FileId.Identifier,
    })
}

#[cfg(unix)]
fn metadata_identity(metadata: cap_std::fs::Metadata) -> Identity {
    use cap_std::fs::MetadataExt;
    let mut file = [0; 16];
    file[..8].copy_from_slice(&metadata.ino().to_le_bytes());
    Identity {
        volume: metadata.dev(),
        file,
    }
}

fn directory_identity(directory: &Dir) -> Result<Identity> {
    #[cfg(windows)]
    {
        identity(directory)
    }
    #[cfg(unix)]
    {
        Ok(metadata_identity(
            directory.dir_metadata().map_err(|_| refused())?,
        ))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = directory;
        Err(refused())
    }
}

fn file_identity(file: &File) -> Result<Identity> {
    #[cfg(windows)]
    {
        identity(file)
    }
    #[cfg(unix)]
    {
        Ok(metadata_identity(file.metadata().map_err(|_| refused())?))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(refused())
    }
}

fn root(path: &Path) -> Result<(Root, Dir)> {
    if !path.is_absolute() {
        return Err(refused());
    }
    let directory = files::open_root(path)?;
    let identity = directory_identity(&directory)?;
    let path = path.canonicalize().map_err(|_| refused())?;
    if directory_identity(&files::open_root(&path)?)? != identity {
        return Err(refused());
    }
    Ok((Root { path, identity }, directory))
}

struct SourceLock {
    _file: std::fs::File,
    identity: Identity,
}

fn writer_lock(root: &Dir) -> Result<SourceLock> {
    let file = backup::opened(root, ".vivac/lock")?;
    let identity = file_identity(&file)?;
    let file = file.into_std();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match file.try_lock_shared() {
            Ok(()) => {
                return Ok(SourceLock {
                    _file: file,
                    identity,
                })
            }
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(_) => return Err(refused()),
        }
    }
}

fn hash(file: &mut File) -> Result<(u64, String)> {
    file.seek(SeekFrom::Start(0)).map_err(|_| refused())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; backup::BUFFER];
    let mut size = 0u64;
    loop {
        let count = file.read(&mut buffer).map_err(|_| refused())?;
        if count == 0 {
            break;
        }
        size = size.checked_add(count as u64).ok_or_else(refused)?;
        if size > backup::MAX_FILE {
            return Err(refused());
        }
        hasher.update(&buffer[..count]);
    }
    file.seek(SeekFrom::Start(0)).map_err(|_| refused())?;
    Ok((size, format!("{:x}", hasher.finalize())))
}

fn member(root: &Dir, lane: &str, path: &str) -> Result<(Member, File)> {
    let mut file = backup::opened(root, path)?;
    let identity = file_identity(&file)?;
    let length = file.metadata().map_err(|_| refused())?.len();
    if (path == ".vivac/config" && length > backup::MAX_CONFIG)
        || (path == ".vivac/lane" && length > backup::MAX_MARKER)
    {
        return Err(refused());
    }
    let (size, sha256) = hash(&mut file)?;
    Ok((
        Member {
            root: lane.into(),
            path: path.into(),
            size,
            sha256,
            identity,
        },
        file,
    ))
}

fn marker(root: &Dir, identity: &str) -> Result<Option<(String, Member, File)>> {
    let (member, mut file) = match member(root, "", ".vivac/lane") {
        Err(crate::failure::Failure::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None)
        }
        result => result?,
    };
    let bytes = backup::metadata_bytes(&mut file, backup::MAX_MARKER, "Invalid metadata.")?;
    json::decode(&bytes)?;
    let lane = backup::marker_file(&mut file, identity)?;
    Ok(Some((lane, member, file)))
}

fn target(path: &Path) -> Result<(PathBuf, Dir)> {
    if !path.is_absolute() {
        return Err(refused());
    }
    let parent = path
        .parent()
        .ok_or_else(refused)?
        .canonicalize()
        .map_err(|_| refused())?;
    let path = parent.join(path.file_name().ok_or_else(refused)?);
    let (directory, _) = backup::destination(&path)?;
    Ok((path, directory))
}

fn no_overlap(destination: &Path, roots: &BTreeMap<String, Root>) -> Result<()> {
    for root in roots.values() {
        if destination.starts_with(&root.path) || root.path.starts_with(destination) {
            return Err(refused());
        }
    }
    Ok(())
}

pub(super) fn capture(mut request: Request) -> Result<CapturedPlan> {
    let (source, directory) = root(&request.source_root)?;
    let lock = writer_lock(&directory)?;
    let (lock_member, lock_handle) = member(&directory, "", ".vivac/lock")?;
    if lock_member.identity != lock.identity {
        return Err(refused());
    }
    let (mut log_member, mut log) = member(&directory, "", ".vivac/events")?;
    let (mut config_member, mut config) = member(&directory, "", ".vivac/config")?;
    let bytes = backup::metadata_bytes(&mut config, backup::MAX_CONFIG, "Invalid metadata.")?;
    json::decode(&bytes)?;
    backup::configuration(&mut config)?;
    let configuration: crate::store::Config =
        serde_json::from_slice(&bytes).map_err(|_| refused())?;
    let (source_tree, tree, locations) = history::read(
        &mut BufReader::with_capacity(backup::BUFFER, &mut log),
        &request.selection,
    )?;
    let owner_marker = marker(&directory, &source_tree)?;
    if owner_marker.is_none() && tree.main_claimed {
        return Err(refused());
    }
    let owner = owner_marker
        .as_ref()
        .map_or("main", |(lane, _, _)| lane)
        .to_owned();
    log_member.root = owner.clone();
    config_member.root = owner.clone();
    let mut lock_member = lock_member;
    lock_member.root = owner.clone();
    let mut members = vec![log_member, config_member, lock_member];
    let mut handles = vec![log, config, lock_handle];
    let mut roots = BTreeMap::from([(owner.clone(), source)]);
    let mut directories = BTreeMap::new();
    if let Some((_, mut member, handle)) = owner_marker {
        member.root = owner.clone();
        members.push(member);
        handles.push(handle);
    }
    directories.insert(owner.clone(), directory);
    for (lane, path) in request.lane_roots {
        backup::lane_id(&lane)?;
        let (root, directory) = root(&path)?;
        if let Some(previous) = roots.get(&lane) {
            if previous.identity != root.identity || previous.path != root.path {
                return Err(refused());
            }
            continue;
        }
        if roots
            .values()
            .any(|previous| previous.identity == root.identity)
        {
            return Err(refused());
        }
        let (actual, mut member, handle) = marker(&directory, &source_tree)?.ok_or_else(refused)?;
        if lane != actual {
            return Err(refused());
        }
        member.root = lane.clone();
        members.push(member);
        handles.push(handle);
        directories.insert(lane.clone(), directory);
        roots.insert(lane, root);
    }
    for lane in roots.keys() {
        if !tree.lanes.contains_key(lane) {
            return Err(refused());
        }
    }
    for lane in backup::assets::required_lanes(&tree)? {
        if !roots.contains_key(&lane) {
            return Err(refused());
        }
    }
    let paths = roots
        .iter()
        .map(|(lane, root)| (lane.clone(), root.path.clone()))
        .collect();
    let custody = backup::assets::collect_with_directories(&tree, &paths, &directories)?;
    let strict = strict_files(&tree);
    for asset in custody {
        let (member, mut file) = member(&directories[&asset.lane], &asset.lane, &asset.path)?;
        if member.sha256 != asset.digest {
            return Err(refused());
        }
        if strict.contains(&(asset.lane.clone(), asset.path.clone())) {
            check_strict(&mut file, &member)?;
        }
        members.push(member);
        handles.push(file);
    }
    drop(tree);
    let (destination, destination_parent) = target(&request.destination)?;
    no_overlap(&destination, &roots)?;
    request.selection.sort();
    let mut identities =
        std::collections::BTreeSet::from([source_tree.clone(), configuration.project_id.clone()]);
    let mut fresh = || loop {
        let id = crate::id::ulid();
        if identities.insert(id.clone()) {
            break id;
        }
    };
    let operation_id = fresh();
    let successor_tree = fresh();
    let successor_project = fresh();
    let successor_actor = format!("a_{}", fresh());
    let plan = CapturedPlan {
        envelope: Envelope {
            version: 1,
            operation_id,
            source_tree,
            source_project: configuration.project_id,
            successor_tree,
            successor_project,
            successor_actor,
            share: false,
            owner_lane: owner,
            roots,
            files: members,
            selection: request.selection,
            locations,
            destination,
            destination_parent: directory_identity(&destination_parent)?,
            external_actions: vec![
                ExternalAction::RestrictSource,
                ExternalAction::ReviewBackupsAndExports,
                ExternalAction::ReviewNativeFilesAndSessions,
                ExternalAction::ReviewRemoteCopies,
                ExternalAction::RetireOtherWriters,
            ],
        },
        pins: Pins {
            roots: directories,
            files: handles,
            destination_parent,
        },
    };
    validate(&plan.envelope)?;
    verify_locked(&plan)?;
    if file_identity(&backup::opened(
        &plan.pins.roots[&plan.envelope.owner_lane],
        ".vivac/lock",
    )?)? != lock.identity
    {
        return Err(refused());
    }
    plan.encode_private()?;
    drop(lock);
    Ok(plan)
}

fn validate(envelope: &Envelope) -> Result<()> {
    if envelope.version != 1
        || envelope.share
        || envelope.files.is_empty()
        || envelope.files.len() > backup::MAX_FILES
        || envelope.selection.is_empty()
        || envelope.locations.is_empty()
    {
        return Err(refused());
    }
    let mut total = 0u64;
    let mut names = std::collections::BTreeSet::new();
    for file in &envelope.files {
        if !envelope.roots.contains_key(&file.root)
            || !names.insert((&file.root, &file.path))
            || file.size > backup::MAX_FILE
            || file.path.split('/').any(|part| {
                part.is_empty() || matches!(part, "." | "..") || part.contains(['\\', ':'])
            })
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(refused());
        }
        total = total.checked_add(file.size).ok_or_else(refused)?;
        if total > backup::MAX_TOTAL {
            return Err(refused());
        }
    }
    if !envelope.roots.contains_key(&envelope.owner_lane) {
        return Err(refused());
    }
    for selector in &envelope.selection {
        if selector.node.is_empty() || crate::redact::check_field("node", &selector.node).is_some()
        {
            return Err(refused());
        }
    }
    for location in &envelope.locations {
        if location.seq == 0
            || !envelope.selection.contains(&Selector {
                node: location.node.clone(),
                role: location.role,
            })
        {
            return Err(refused());
        }
    }
    for id in [
        &envelope.source_tree,
        &envelope.source_project,
        &envelope.operation_id,
        &envelope.successor_tree,
        &envelope.successor_project,
        &envelope.successor_actor,
    ] {
        if id.is_empty() || crate::redact::check_field("identity", id).is_some() {
            return Err(refused());
        }
    }
    let fresh = [
        &envelope.operation_id,
        &envelope.successor_tree,
        &envelope.successor_project,
        &envelope.successor_actor,
    ];
    let mut seen = std::collections::BTreeSet::new();
    for id in fresh {
        if id.is_empty()
            || id == &envelope.source_tree
            || id == &envelope.source_project
            || !seen.insert(id)
        {
            return Err(refused());
        }
    }
    if envelope.selection.windows(2).any(|pair| pair[0] >= pair[1])
        || envelope.locations.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(refused());
    }
    Ok(())
}

pub(super) fn decode(envelope: Envelope) -> Result<CapturedPlan> {
    validate(&envelope)?;
    let mut roots = BTreeMap::new();
    for (lane, saved) in &envelope.roots {
        let (current, directory) = root(&saved.path)?;
        if current.identity != saved.identity {
            return Err(refused());
        }
        roots.insert(lane.clone(), directory);
    }
    let lock = writer_lock(roots.get(&envelope.owner_lane).ok_or_else(refused)?)?;
    let mut files = Vec::new();
    for member in &envelope.files {
        files.push(backup::opened(&roots[&member.root], &member.path)?);
    }
    let (_, destination_parent) = target(&envelope.destination)?;
    let plan = CapturedPlan {
        envelope,
        pins: Pins {
            roots,
            files,
            destination_parent,
        },
    };
    verify_locked(&plan)?;
    if file_identity(&backup::opened(
        &plan.pins.roots[&plan.envelope.owner_lane],
        ".vivac/lock",
    )?)? != lock.identity
    {
        return Err(refused());
    }
    drop(lock);
    Ok(plan)
}

pub(super) fn verify(plan: &CapturedPlan) -> Result<()> {
    let owner = plan
        .pins
        .roots
        .get(&plan.envelope.owner_lane)
        .ok_or_else(refused)?;
    let lock = writer_lock(owner)?;
    if file_identity(&backup::opened(owner, ".vivac/lock")?)? != lock.identity {
        return Err(refused());
    }
    let result = verify_locked(plan);
    if file_identity(&backup::opened(owner, ".vivac/lock")?)? != lock.identity {
        return Err(refused());
    }
    drop(lock);
    result
}

fn verify_locked(plan: &CapturedPlan) -> Result<()> {
    validate(&plan.envelope)?;
    for (lane, saved) in &plan.envelope.roots {
        if directory_identity(&plan.pins.roots[lane])? != saved.identity
            || directory_identity(&files::open_root(&saved.path)?)? != saved.identity
        {
            return Err(refused());
        }
    }
    let (destination, parent) = target(&plan.envelope.destination)?;
    no_overlap(&destination, &plan.envelope.roots)?;
    if directory_identity(&parent)? != plan.envelope.destination_parent
        || directory_identity(&plan.pins.destination_parent)? != plan.envelope.destination_parent
    {
        return Err(refused());
    }
    for (member, pinned) in plan.envelope.files.iter().zip(&plan.pins.files) {
        let mut current = backup::opened(&plan.pins.roots[&member.root], &member.path)?;
        if file_identity(pinned)? != member.identity || file_identity(&current)? != member.identity
        {
            return Err(refused());
        }
        let (size, digest) = hash(&mut current)?;
        if size != member.size || digest != member.sha256 {
            return Err(refused());
        }
    }
    verify_scope(plan)?;
    Ok(())
}

fn verify_scope(plan: &CapturedPlan) -> Result<()> {
    let envelope = &plan.envelope;
    let owner = &plan.pins.roots[&envelope.owner_lane];
    let mut log = backup::opened(owner, ".vivac/events")?;
    let (identity, tree, locations) = history::read(
        &mut BufReader::with_capacity(backup::BUFFER, &mut log),
        &envelope.selection,
    )?;
    if identity != envelope.source_tree || locations != envelope.locations {
        return Err(refused());
    }
    let mut config = backup::opened(owner, ".vivac/config")?;
    let bytes = backup::metadata_bytes(&mut config, backup::MAX_CONFIG, "Invalid metadata.")?;
    json::decode(&bytes)?;
    backup::configuration(&mut config)?;
    let config: crate::store::Config = serde_json::from_slice(&bytes).map_err(|_| refused())?;
    if config.project_id != envelope.source_project || config.actor == envelope.successor_actor {
        return Err(refused());
    }
    let mut required = std::collections::BTreeSet::from([
        (envelope.owner_lane.clone(), ".vivac/events".to_owned()),
        (envelope.owner_lane.clone(), ".vivac/config".to_owned()),
        (envelope.owner_lane.clone(), ".vivac/lock".to_owned()),
    ]);
    for (lane, directory) in &plan.pins.roots {
        if !tree.lanes.contains_key(lane) {
            return Err(refused());
        }
        match marker(directory, &identity)? {
            Some((actual, _, _)) if actual == *lane => {
                required.insert((lane.clone(), ".vivac/lane".to_owned()));
            }
            None if *lane == envelope.owner_lane && lane == "main" && !tree.main_claimed => {}
            _ => return Err(refused()),
        }
    }
    let paths = envelope
        .roots
        .iter()
        .map(|(lane, root)| (lane.clone(), root.path.clone()))
        .collect();
    let custody = backup::assets::collect_with_directories(&tree, &paths, &plan.pins.roots)?;
    let strict = strict_files(&tree);
    for asset in custody {
        if strict.contains(&(asset.lane.clone(), asset.path.clone())) {
            let member = envelope
                .files
                .iter()
                .find(|member| member.root == asset.lane && member.path == asset.path)
                .ok_or_else(refused)?;
            let mut file = backup::opened(&plan.pins.roots[&asset.lane], &asset.path)?;
            check_strict(&mut file, member)?;
        }
        required.insert((asset.lane, asset.path));
    }
    let captured = envelope
        .files
        .iter()
        .map(|file| (file.root.clone(), file.path.clone()))
        .collect();
    if required != captured {
        return Err(refused());
    }
    Ok(())
}

fn strict_files(tree: &crate::model::Tree) -> std::collections::BTreeSet<(String, String)> {
    tree.artifact_history
        .iter()
        .filter_map(|(lane, body)| match body {
            crate::event::Body::ArtifactCaptured { object, .. }
            | crate::event::Body::ArtifactCapturedV2 { object, .. } => {
                Some((lane.clone(), format!(".vivac-artifacts/{object}")))
            }
            _ => None,
        })
        .collect()
}

fn check_strict(file: &mut File, member: &Member) -> Result<()> {
    if file_identity(file)? != member.identity {
        return Err(refused());
    }
    file.seek(SeekFrom::Start(0)).map_err(|_| refused())?;
    let receipt =
        crate::artifacts::guard::copy_checked(file, std::io::sink()).map_err(|_| refused())?;
    if receipt.size != member.size || receipt.digest != member.sha256 {
        return Err(refused());
    }
    Ok(())
}
