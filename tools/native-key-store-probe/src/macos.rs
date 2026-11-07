use std::path::Path;

use core_foundation::data::CFData;
use security_framework::{
    base::Error,
    item::{
        ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, Limit, Location, SearchResult,
    },
    os::macos::keychain::{CreateOptions, KeychainSettings, SecKeychain},
};
use security_framework_sys::base::{errSecDuplicateItem, errSecItemNotFound};
use zeroize::Zeroizing;

use crate::PASSWORD;
const SERVICE: &str = "vivac.native-key-store-probe.fixture.v1";
const ACCOUNT: &str = "synthetic-signing-seed";
// Apple's errSecInteractionNotAllowed is not exported by security-framework-sys.
const INTERACTION_NOT_ALLOWED: i32 = -25308;
// Apple's errSecNoSuchKeychain is not exported by security-framework-sys.
const NO_SUCH_KEYCHAIN: i32 = -25294;

fn create(path: &Path) -> Result<SecKeychain, crate::BoxError> {
    eprintln!("probe step: create-keychain-before");
    // macOS can append -db to the supplied keychain path.
    for candidate in [path.to_path_buf(), path.with_extension("keychain-db")] {
        match candidate.symlink_metadata() {
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "refusing to replace an existing keychain",
                )
                .into());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let keychain = CreateOptions::new()
        .password(PASSWORD)
        .prompt_user(false)
        .create(path)?;
    eprintln!("probe step: create-keychain-after");
    Ok(keychain)
}

fn open(root: &Path) -> Result<SecKeychain, crate::BoxError> {
    let mut keychain = SecKeychain::open(root.join("probe.keychain"))?;
    eprintln!("probe step: unlock-before");
    keychain.unlock(Some(PASSWORD))?;
    eprintln!("probe step: unlock-after");
    Ok(keychain)
}

fn search(keychain: &SecKeychain, account: &str) -> ItemSearchOptions {
    let mut query = ItemSearchOptions::new();
    query
        .keychains(std::slice::from_ref(keychain))
        .class(ItemClass::generic_password())
        .service(SERVICE)
        .account(account)
        .load_data(true)
        .limit(Limit::All);
    query
}

fn read(keychain: &SecKeychain) -> Result<Zeroizing<Vec<u8>>, crate::BoxError> {
    eprintln!("probe step: read-before");
    let mut results = search(keychain, ACCOUNT).search()?;
    if results.len() != 1 {
        return Err("expected exactly one fixture item".into());
    }
    match results.pop() {
        Some(SearchResult::Data(data)) => {
            eprintln!("probe step: read-after");
            Ok(Zeroizing::new(data))
        }
        _ => Err("expected fixture data".into()),
    }
}

fn add(keychain: &SecKeychain, data: &[u8]) -> Result<(), Error> {
    eprintln!("probe step: add-before");
    ItemAddOptions::new(ItemAddValue::Data {
        class: ItemClass::generic_password(),
        data: CFData::from_buffer(data),
    })
    .set_location(Location::FileKeychain(keychain.clone()))
    .set_service(SERVICE)
    .set_account_name(ACCOUNT)
    .add()?;
    eprintln!("probe step: add-after");
    Ok(())
}

fn verify(root: &Path, data: &[u8]) -> Result<(), crate::BoxError> {
    eprintln!("probe step: verify-before");
    crate::verify_payload(root, data)?;
    eprintln!("probe step: verify-after");
    Ok(())
}

fn expect_missing(keychain: &SecKeychain, account: &str) -> Result<(), crate::BoxError> {
    match search(keychain, account).search() {
        Err(error) if error.code() == errSecItemNotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("unexpected fixture lookup success".into()),
    }
}

pub fn run(root: &Path, stage: &str) -> Result<(), crate::BoxError> {
    // HOME does not isolate Keychain Services. Only disposable hosted runners qualify.
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("macOS probe requires a disposable GitHub-hosted runner".into());
    }
    let _interaction = SecKeychain::disable_user_interaction()?;
    match stage {
        "missing" => {
            let path = root.join("missing.keychain");
            for candidate in [path.clone(), path.with_extension("keychain-db")] {
                match candidate.symlink_metadata() {
                    Ok(_) => return Err("missing keychain fixture already exists".into()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            eprintln!("probe step: open-missing-before");
            let result = SecKeychain::open(path).and_then(|mut keychain| {
                eprintln!("probe step: open-missing-after");
                eprintln!("probe step: unlock-missing-before");
                keychain.unlock(Some(PASSWORD))?;
                eprintln!("probe step: unlock-missing-after");
                Ok(())
            });
            match result {
                Err(error) if error.code() == NO_SUCH_KEYCHAIN => {}
                Err(error) => return Err(error.into()),
                Ok(()) => return Err("missing keychain unlock unexpectedly succeeded".into()),
            }
        }
        "create" => {
            let mut keychain = create(&root.join("probe.keychain"))?;
            eprintln!("probe step: unlock-before");
            keychain.unlock(Some(PASSWORD))?;
            eprintln!("probe step: unlock-after");
            let payload = crate::payload()?;
            add(&keychain, &payload)?;
            eprintln!("probe step: save-public-before");
            crate::save_public(root, &payload)?;
            eprintln!("probe step: save-public-after");
            verify(root, &read(&keychain)?)?;
        }
        "read" => verify(root, &read(&open(root)?)?)?,
        "existing" => {
            let original = read(&open(root)?)?;
            verify(root, &original)?;
            match create(&root.join("probe.keychain")) {
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists) => {
                }
                Err(error) => return Err(error),
                Ok(_) => return Err("existing fixture keychain was replaced".into()),
            }
            let after = read(&open(root)?)?;
            if *original != *after {
                return Err("existing fixture keychain changed".into());
            }
            verify(root, &after)?;
        }
        "duplicate" => {
            let keychain = open(root)?;
            let original = read(&keychain)?;
            crate::verify_payload(root, &original)?;
            let replacement = crate::payload()?;
            match add(&keychain, &replacement) {
                Err(error) if error.code() == errSecDuplicateItem => {}
                Err(error) => return Err(error.into()),
                Ok(()) => return Err("duplicate fixture was accepted".into()),
            }
            let after = read(&keychain)?;
            if *original != *after {
                return Err("duplicate attempt changed the fixture".into());
            }
            crate::verify_payload(root, &after)?;
        }
        "wrong" => {
            let keychain = open(root)?;
            crate::verify_payload(root, &read(&keychain)?)?;
            expect_missing(&keychain, "different-synthetic-account")?;
            let mut other = create(&root.join("other.keychain"))?;
            other.unlock(Some(PASSWORD))?;
            expect_missing(&other, ACCOUNT)?;
            crate::verify_payload(root, &read(&keychain)?)?;
        }
        "locked" => {
            let mut keychain = open(root)?;
            crate::verify_payload(root, &read(&keychain)?)?;
            let mut settings = KeychainSettings::new();
            settings.set_lock_interval(Some(1));
            keychain.set_settings(&settings)?;
            // Exercise the native inactivity lock through maintained wrappers.
            // No secret lookup occurs while the one-second interval elapses.
            std::thread::sleep(std::time::Duration::from_secs(2));
            match search(&keychain, ACCOUNT).search() {
                Err(error) if error.code() == INTERACTION_NOT_ALLOWED => {}
                Err(error) => return Err(error.into()),
                Ok(_) => return Err("inactivity-locked keychain returned fixture data".into()),
            }
            keychain.unlock(Some(PASSWORD))?;
            settings.set_lock_interval(None);
            keychain.set_settings(&settings)?;
            crate::verify_payload(root, &read(&keychain)?)?;
        }
        "corrupt" => {
            let keychain = open(root)?;
            crate::verify_payload(root, &read(&keychain)?)?;
            // Delete only the explicit fixture; no default keychain is queried.
            let mut query = ItemSearchOptions::new();
            query
                .keychains(std::slice::from_ref(&keychain))
                .class(ItemClass::generic_password())
                .service(SERVICE)
                .account(ACCOUNT);
            query.delete()?;
            add(&keychain, &[0_u8; 31])?;
            let malformed = read(&keychain)?;
            if malformed.len() != 31 || crate::verify_payload(root, &malformed).is_ok() {
                return Err("malformed fixture was not rejected".into());
            }
        }
        _ => return Err("unknown macOS probe stage".into()),
    }
    Ok(())
}
