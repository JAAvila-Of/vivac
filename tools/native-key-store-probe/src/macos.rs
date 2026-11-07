use std::path::Path;

use core_foundation::{base::TCFType, data::CFData};
use security_framework::{
    base::Error,
    item::{
        ItemAddOptions, ItemAddValue, ItemClass, ItemSearchOptions, Limit, Location, Reference,
        SearchResult,
    },
    os::macos::{
        keychain::{CreateOptions, SecKeychain},
        keychain_item::SecKeychainItem,
    },
};
use security_framework_sys::base::{errSecAuthFailed, errSecDuplicateItem, errSecItemNotFound};
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
        .account(account);
    query
}

fn search_refs(keychain: &SecKeychain, account: &str) -> ItemSearchOptions {
    let mut query = search(keychain, account);
    query.load_refs(true).limit(Limit::All);
    query
}

fn search_data(keychain: &SecKeychain) -> ItemSearchOptions {
    let mut query = search(keychain, ACCOUNT);
    // Password data cannot be requested with kSecMatchLimitAll on macOS.
    query.load_data(true).limit(1);
    query
}

fn unique_reference(keychain: &SecKeychain) -> Result<SecKeychainItem, crate::BoxError> {
    let mut results = search_refs(keychain, ACCOUNT).search()?;
    if results.len() != 1 {
        return Err("expected exactly one fixture reference".into());
    }
    match results.pop() {
        Some(SearchResult::Ref(Reference::KeychainItem(item))) => Ok(item),
        _ => Err("expected a native fixture item reference".into()),
    }
}

fn read(keychain: &SecKeychain) -> Result<Zeroizing<Vec<u8>>, crate::BoxError> {
    eprintln!("probe step: read-before");
    let before = unique_reference(keychain)?;
    let mut results = search_data(keychain).search()?;
    if results.len() != 1 {
        return Err("expected exactly one fixture item".into());
    }
    let data = match results.pop() {
        Some(SearchResult::Data(data)) => Zeroizing::new(data),
        _ => return Err("expected fixture data".into()),
    };
    let after = unique_reference(keychain)?;
    // CFType's maintained PartialEq delegates to CFEqual, not pointer equality.
    if before.as_CFType() != after.as_CFType() {
        return Err("fixture item identity changed during lookup".into());
    }
    eprintln!("probe step: read-after");
    Ok(data)
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
    match search_refs(keychain, account).search() {
        Err(error) if error.code() == errSecItemNotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("unexpected fixture lookup success".into()),
    }
}

// This probe deliberately exercises a temporary classic file keychain, whose
// explicit lock and status APIs are deprecated by Apple but remain SDK-bound.
#[allow(deprecated)]
fn probe_locked(root: &Path) -> Result<(), crate::BoxError> {
    let mut keychain = open(root)?;
    verify(root, &read(&keychain)?)?;
    // SAFETY: both wrappers represent Apple's same SecKeychain CF object. The
    // security-framework owner retains it throughout this function; the borrowed
    // SDK view never escapes or takes ownership. Reject null rather than using
    // None, which would select the user's default keychain.
    let native = unsafe {
        keychain
            .as_concrete_TypeRef()
            .cast::<objc2_security::SecKeychain>()
            .as_ref()
    }
    .ok_or("fixture keychain reference is null")?;
    let get_status = || -> Result<u32, crate::BoxError> {
        let mut status: objc2_security::SecKeychainStatus = 0;
        eprintln!("probe step: get-status-before");
        // SAFETY: native is the retained fixture above; the output points to a
        // live, initialized status value for the complete synchronous call.
        let result = unsafe {
            objc2_security::SecKeychain::status(Some(native), std::ptr::NonNull::from(&mut status))
        };
        if result != 0 {
            return Err(Error::from_code(result).into());
        }
        eprintln!("probe step: get-status-after");
        Ok(status)
    };
    if get_status()? & objc2_security::kSecUnlockStateStatus == 0 {
        return Err("fixture keychain was not initially unlocked".into());
    }
    eprintln!("probe step: lock-before");
    // SAFETY: native borrows only the retained, non-null fixture keychain.
    let result = unsafe { objc2_security::SecKeychain::lock(Some(native)) };
    if result != 0 {
        return Err(Error::from_code(result).into());
    }
    eprintln!("probe step: lock-after");
    if get_status()? & objc2_security::kSecUnlockStateStatus != 0 {
        return Err("fixture keychain remained unlocked after native lock".into());
    }
    match search_data(&keychain).search() {
        Err(error) if matches!(error.code(), INTERACTION_NOT_ALLOWED | errSecAuthFailed) => {}
        Err(error) => return Err(error.into()),
        Ok(_) => return Err("locked keychain returned fixture data".into()),
    }
    eprintln!("probe step: unlock-before");
    keychain.unlock(Some(PASSWORD))?;
    eprintln!("probe step: unlock-after");
    if get_status()? & objc2_security::kSecUnlockStateStatus == 0 {
        return Err("fixture keychain remained locked after unlock".into());
    }
    verify(root, &read(&keychain)?)
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
        "locked" => probe_locked(root)?,
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
