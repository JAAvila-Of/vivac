#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(any(target_os = "linux", target_os = "macos", test))]
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
#[cfg(any(target_os = "linux", target_os = "macos", test))]
use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
use zeroize::Zeroizing;

type BoxError = Box<dyn std::error::Error>;
#[cfg(target_os = "macos")]
const PASSWORD: &str = "vivac-native-probe-fixture-only-2026";
#[cfg(any(target_os = "linux", target_os = "macos", test))]
const HEADER: &[u8] = b"vivac.native-store.probe.v1\0";
#[cfg(any(target_os = "linux", target_os = "macos", test))]
const MESSAGE: &[u8] = b"vivac.native-store.signature-fixture.v1\0";

fn main() {
    if let Err(error) = run() {
        if let Some(error) = error.downcast_ref::<std::io::Error>() {
            eprintln!("probe I/O kind: {:?}", error.kind());
        }
        #[cfg(target_os = "macos")]
        if let Some(error) = error.downcast_ref::<security_framework::base::Error>() {
            eprintln!("macOS status: {}", error.code());
        }
        eprintln!("Native key-store probe refused or failed; no result is accredited.");
        std::process::exit(1);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn run() -> Result<(), BoxError> {
    Err("native provider probes require Linux or macOS".into())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run() -> Result<(), BoxError> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("expected an isolated root and one probe stage".into());
    }
    let root = Path::new(&args[0]);
    eprintln!("probe step: validating isolated root");
    validate_root(root)?;
    eprintln!("probe step: root validated");
    let stage = args[1].to_str().ok_or("invalid stage")?;
    if !matches!(
        stage,
        "create"
            | "read"
            | "ready"
            | "missing"
            | "wrong"
            | "locked"
            | "corrupt"
            | "ambiguous"
            | "duplicate"
            | "existing"
    ) {
        return Err("unsupported stage".into());
    }
    #[cfg(target_os = "linux")]
    linux::run(root, stage)?;
    #[cfg(target_os = "macos")]
    macos::run(root, stage)?;
    println!("native key-store probe passed: {stage}");
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn validate_root(root: &Path) -> Result<(), BoxError> {
    use std::os::unix::fs::MetadataExt;
    let canonical = root.canonicalize()?;
    let temporary = std::env::temp_dir().canonicalize()?;
    if !root.is_absolute()
        || canonical != root
        || !canonical.starts_with(&temporary)
        || canonical == temporary
    {
        return Err("a canonical isolated temporary root is required".into());
    }
    let metadata = std::fs::symlink_metadata(root)?;
    // SAFETY: geteuid takes no arguments and returns this process's effective identity.
    if !metadata.is_dir()
        || metadata.mode() & 0o777 != 0o700
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err("the temporary root must be private and owned by the probe user".into());
    }
    let marker = std::fs::symlink_metadata(root.join(".probe-owned"))?;
    if !marker.is_file()
        || marker.nlink() != 1
        || std::fs::read(root.join(".probe-owned"))? != b"vivac.native-key-store-probe.v1\n"
    {
        return Err("the harness ownership marker is absent".into());
    }
    #[cfg(target_os = "linux")]
    {
        for (key, leaf) in [
            ("HOME", "home"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_RUNTIME_DIR", "runtime"),
        ] {
            if std::env::var_os(key).as_deref() != Some(root.join(leaf).as_os_str()) {
                return Err("the Linux probe requires its own profile".into());
            }
        }
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS")?;
        let wanted = format!("unix:path={}", root.join("bus").display());
        if address.split(',').next() != Some(wanted.as_str()) || address.contains(';') {
            return Err("the Linux probe requires its own session bus".into());
        }
    }
    #[cfg(target_os = "macos")]
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
        || std::env::var("RUNNER_ENVIRONMENT").as_deref() != Ok("github-hosted")
    {
        return Err("the macOS probe is restricted to a disposable hosted runner".into());
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn payload() -> Result<Zeroizing<Vec<u8>>, BoxError> {
    let mut value = Zeroizing::new(vec![0u8; HEADER.len() + 32]);
    value[..HEADER.len()].copy_from_slice(HEADER);
    getrandom::getrandom(&mut value[HEADER.len()..])
        .map_err(|_| "operating-system randomness is unavailable")?;
    Ok(value)
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn key(value: &[u8]) -> Result<SigningKey, BoxError> {
    if value.len() != HEADER.len() + 32 || !value.starts_with(HEADER) {
        return Err("the synthetic key payload is malformed".into());
    }
    let seed: &[u8; 32] = value[HEADER.len()..].try_into()?;
    Ok(SigningKey::from_bytes(seed))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn save_public(root: &Path, value: &[u8]) -> Result<(), BoxError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("expected-public.bin"))?;
    file.write_all(key(value)?.verifying_key().as_bytes())?;
    file.sync_all()?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn verify_payload(root: &Path, value: &[u8]) -> Result<(), BoxError> {
    let signing = key(value)?;
    let public: [u8; 32] = std::fs::read(root.join("expected-public.bin"))?
        .try_into()
        .map_err(|_| "the public fixture receipt has the wrong size")?;
    let verifying = VerifyingKey::from_bytes(&public)?;
    if verifying.is_weak() || signing.verifying_key() != verifying {
        return Err("the native store returned a different key".into());
    }
    let signature = signing.sign(MESSAGE);
    verifying.verify_strict(MESSAGE, &signature)?;
    let mut changed = MESSAGE.to_vec();
    changed[0] ^= 1;
    if verifying.verify_strict(&changed, &signature).is_ok() {
        return Err("the signature accepted changed bytes".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_payloads_are_rejected_before_signing() {
        let valid = payload().unwrap();
        assert!(key(&valid).is_ok());
        for size in 0..valid.len() {
            assert!(key(&valid[..size]).is_err());
        }
        let mut extended = valid.to_vec();
        extended.push(0);
        assert!(key(&extended).is_err());
        for index in 0..HEADER.len() {
            let mut changed = Zeroizing::new(valid.to_vec());
            changed[index] ^= 1;
            assert!(key(&changed).is_err());
        }
    }

    #[test]
    fn independent_generated_credentials_do_not_share_a_public_identity() {
        let first = key(&payload().unwrap()).unwrap();
        let second = key(&payload().unwrap()).unwrap();
        assert_ne!(first.verifying_key(), second.verifying_key());
        let signature = first.sign(MESSAGE);
        first
            .verifying_key()
            .verify_strict(MESSAGE, &signature)
            .unwrap();
        assert!(
            second
                .verifying_key()
                .verify_strict(MESSAGE, &signature)
                .is_err()
        );
    }

    #[test]
    fn public_receipt_binding_rejects_another_seed() {
        let root = std::env::temp_dir().join(format!("vivac-probe-unit-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let first = payload().unwrap();
        std::fs::write(
            root.join("expected-public.bin"),
            key(&first).unwrap().verifying_key().as_bytes(),
        )
        .unwrap();
        assert!(verify_payload(&root, &first).is_ok());
        assert!(verify_payload(&root, &payload().unwrap()).is_err());
        std::fs::write(root.join("expected-public.bin"), [0u8; 31]).unwrap();
        assert!(verify_payload(&root, &first).is_err());
        std::fs::remove_file(root.join("expected-public.bin")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
