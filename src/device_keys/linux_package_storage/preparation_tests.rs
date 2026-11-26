use super::*;
use ed25519_dalek::SigningKey;
use ssh_key::{
    private::{Ed25519Keypair, Ed25519PrivateKey, KeypairData},
    public::Ed25519PublicKey,
    Cipher, Kdf, LineEnding, PrivateKey,
};
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt};
use std::path::PathBuf;
use std::sync::OnceLock;

const COMMENT: &str = "vivac.device-key.v1:00000000000000000000000001:00000000000000000000000002";

fn fixture(alternate: bool) -> &'static [u8] {
    static ORIGINAL: OnceLock<Vec<u8>> = OnceLock::new();
    static ALTERNATE: OnceLock<Vec<u8>> = OnceLock::new();
    let storage = if alternate { &ALTERNATE } else { &ORIGINAL };
    storage.get_or_init(|| {
        let seed = [if alternate { 42 } else { 41 }; 32];
        let signing = SigningKey::from_bytes(&seed);
        let pair = Ed25519Keypair {
            private: Ed25519PrivateKey::from_bytes(&seed),
            public: Ed25519PublicKey(signing.verifying_key().to_bytes()),
        };
        PrivateKey::new(KeypairData::Ed25519(pair), COMMENT)
            .expect("public fixture key")
            .encrypt_with(
                Cipher::Aes256Gcm,
                Kdf::Bcrypt {
                    salt: vec![7; 16],
                    rounds: 64,
                },
                1,
                b"public-preparation-fixture-password",
            )
            .expect("public fixture encryption")
            .to_openssh(LineEnding::LF)
            .expect("public fixture encoding")
            .as_bytes()
            .to_vec()
    })
}

struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        let base = PathBuf::from(std::env::var_os("TMPDIR").expect("isolated TMPDIR is required"));
        assert!(
            base.is_absolute(),
            "TMPDIR must be an absolute isolated path"
        );
        let mut nonce = [0; 16];
        getrandom::getrandom(&mut nonce).expect("test nonce");
        let path = base.join(format!("prepared-package-test-{}", hex(&nonce)));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("isolated test root");
        let root = Self(path);
        let held =
            Dir::open_ambient_dir(&root.0, cap_std::ambient_authority()).expect("test root handle");
        assert!(
            private_directory(&held).is_ok(),
            "test requires private ext4 without ACLs"
        );
        root
    }

    fn destination(&self) -> PathBuf {
        self.0.join("package")
    }

    fn prepare(&self) -> PreparedPackage {
        prepare(
            Location::preflight(&self.destination()).expect("preflight"),
            COMMENT,
            fixture(false),
        )
        .unwrap_or_else(|_| panic!("prepare public fixture"))
    }

    fn stage(&self) -> PathBuf {
        let entries: Vec<_> = std::fs::read_dir(&self.0)
            .expect("list test root")
            .map(|entry| entry.expect("test entry").path())
            .filter(|path| {
                path.file_name()
                    .expect("stage name")
                    .as_bytes()
                    .starts_with(PREPARING.as_bytes())
            })
            .collect();
        assert_eq!(entries.len(), 1);
        entries[0].clone()
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove isolated test root");
    }
}

#[test]
fn experimental_path_preparation_cannot_become_a_creation_draft() {
    let root = Root::new();
    let prepared = root.prepare();
    let stage = root.stage();
    assert!(matches!(
        prepared.into_creation_draft(),
        Err(CreateError::Refused)
    ));
    assert!(!stage.exists());
    assert!(!root.destination().exists());
}

#[test]
fn preparation_stays_private_and_drop_removes_only_unchanged_stage() {
    let root = Root::new();
    let prepared = root.prepare();
    let stage = root.stage();
    assert!(!root.destination().exists());
    assert_eq!(
        std::fs::read(stage.join(KEY)).expect("prepared key"),
        fixture(false)
    );
    drop(prepared);
    assert!(!stage.exists());
    assert!(!root.destination().exists());
}

#[test]
fn dropping_a_changed_stage_preserves_all_its_members() {
    let root = Root::new();
    let prepared = root.prepare();
    let stage = root.stage();
    std::fs::write(stage.join(KEY), b"changed after preparation").expect("changed retained member");
    drop(prepared);
    assert_eq!(
        std::fs::read(stage.join(KEY)).expect("changed member preserved"),
        b"changed after preparation"
    );
    assert!(stage.join(MANIFEST).exists());
    assert!(!root.destination().exists());
}

#[test]
fn preparation_does_not_hit_the_before_publication_fault() {
    let root = Root::new();
    let mut stages = Vec::new();
    let prepared = prepare_observed(
        Location::preflight(&root.destination()).expect("preflight"),
        COMMENT,
        fixture(false),
        |stage| {
            stages.push(stage);
            Ok(())
        },
    )
    .unwrap_or_else(|_| panic!("before-publication fault must not run during preparation"));
    assert_eq!(
        stages,
        [
            PackagePrepareStage::KeySynced,
            PackagePrepareStage::ManifestSynced
        ]
    );
    assert!(!root.destination().exists());
    assert_eq!(
        prepared.publish_observed(&mut |stage| {
            assert_eq!(stage, PackagePublishStage::BeforeRename);
            Err(())
        }),
        Err(CreateError::Refused)
    );
    assert_eq!(
        std::fs::read_dir(&root.0)
            .expect("root after refused publication")
            .count(),
        0
    );
}

#[test]
fn replacing_the_stage_name_never_adopts_or_removes_a_foreign_directory() {
    let root = Root::new();
    let prepared = root.prepare();
    let stage = root.stage();
    let retained = root.0.join("retained-stage");
    std::fs::rename(&stage, &retained).expect("retain original stage object");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&stage)
        .expect("foreign stage object");
    std::fs::write(stage.join("foreign"), b"foreign").expect("foreign stage contents");
    assert_eq!(prepared.publish(), Err(CreateError::Refused));
    assert_eq!(
        std::fs::read(stage.join("foreign")).expect("foreign stage preserved"),
        b"foreign"
    );
    assert_eq!(
        std::fs::read(retained.join(KEY)).expect("original stage preserved"),
        fixture(false)
    );
    assert!(retained.join(MANIFEST).exists());
    assert!(!root.destination().exists());
}

#[test]
fn consuming_publication_preserves_the_original_package() {
    let root = Root::new();
    let prepared = root.prepare();
    let stage = root.stage();
    let manifest = std::fs::read(stage.join(MANIFEST)).expect("prepared manifest");
    assert_eq!(prepared.publish(), Ok(()));
    assert!(!stage.exists());
    assert_eq!(
        std::fs::read(root.destination().join(KEY)).expect("published key"),
        fixture(false)
    );
    assert_eq!(
        std::fs::read(root.destination().join(MANIFEST)).expect("published manifest"),
        manifest
    );
    assert!(check(&root.destination()).is_ok());
}

#[test]
fn retained_publication_preserves_original_objects_after_rename_and_drop() {
    let root = Root::new();
    let prepared = root.prepare();
    let stage = root.stage();
    let key = prepared.key.snapshot.identity;
    let manifest = prepared.encoded_manifest.clone();
    let published = prepared
        .publish_retained()
        .unwrap_or_else(|_| panic!("publish retained package"));
    assert!(!stage.exists(), "publication must rename the private stage");
    assert!(published.preparation.committed);
    assert!(published.key.snapshot.identity == key);
    assert!(published
        .revalidate(&published.preparation.location.name)
        .is_ok());
    assert!(published.revalidate_published().is_ok());
    assert!(
        Identity::of(
            &published
                .preparation
                .location
                .parent()
                .symlink_metadata("package")
                .expect("final package")
        ) == published.preparation.identity
    );
    drop(published);
    assert_eq!(
        std::fs::read(root.destination().join(KEY)).expect("published key retained"),
        fixture(false)
    );
    assert_eq!(
        std::fs::read(root.destination().join(MANIFEST)).expect("published manifest retained"),
        manifest
    );
}

#[test]
fn retained_publication_rejects_later_mutation_without_removing_published_objects() {
    let root = Root::new();
    let published = root
        .prepare()
        .publish_retained()
        .unwrap_or_else(|_| panic!("publish retained package"));
    std::fs::write(root.destination().join(KEY), b"changed published member")
        .expect("mutate published member");
    assert!(published
        .revalidate(&published.preparation.location.name)
        .is_err());
    assert!(published.revalidate_published().is_err());
    drop(published);
    assert_eq!(
        std::fs::read(root.destination().join(KEY)).expect("changed published member retained"),
        b"changed published member"
    );
    assert!(root.destination().join(MANIFEST).exists());
}

#[test]
fn publication_checkpoint_refusal_before_rename_leaves_no_final_package() {
    let root = Root::new();
    let prepared = root.prepare();
    let stage = root.stage();
    let mut seen = Vec::new();
    let result = prepared.publish_retained_with(&mut |point| {
        seen.push(point);
        assert_eq!(point, PackagePublishStage::BeforeRename);
        assert!(stage.exists());
        assert!(!root.destination().exists());
        Err(())
    });
    assert_eq!(result.map(|_| ()), Err(CreateError::Refused));
    assert_eq!(seen, [PackagePublishStage::BeforeRename]);
    assert!(!root.destination().exists());
    assert!(!stage.exists());
}

#[test]
fn publication_checkpoint_refusal_after_rename_preserves_the_original_package() {
    let stages = [
        PackagePublishStage::BeforeRename,
        PackagePublishStage::Renamed,
        PackagePublishStage::ParentSynced,
    ];
    for (index, &cut) in stages.iter().enumerate().skip(1) {
        let root = Root::new();
        let prepared = root.prepare();
        let stage = root.stage();
        let directory_identity = prepared.preparation.identity;
        let key_identity = prepared.key.snapshot.identity;
        let manifest = prepared.encoded_manifest.clone();
        let parent = Dir::open_ambient_dir(&root.0, cap_std::ambient_authority())
            .expect("retained test parent");
        let mut seen = Vec::new();
        let result = prepared.publish_retained_with(&mut |point| {
            seen.push(point);
            if point == PackagePublishStage::BeforeRename {
                assert!(stage.exists());
                assert!(!root.destination().exists());
                return Ok(());
            }
            assert!(!stage.exists());
            assert!(
                Identity::of(
                    &parent
                        .symlink_metadata("package")
                        .expect("published directory")
                ) == directory_identity
            );
            let directory = parent
                .open_dir_nofollow("package")
                .expect("published directory handle");
            assert!(
                Identity::of(&directory.symlink_metadata(KEY).expect("published key"))
                    == key_identity
            );
            if point == cut {
                Err(())
            } else {
                Ok(())
            }
        });
        assert_eq!(result.map(|_| ()), Err(CreateError::CommittedIndeterminate));
        assert_eq!(seen, stages[..=index]);
        assert_eq!(
            std::fs::read(root.destination().join(KEY)).expect("original key retained"),
            fixture(false)
        );
        assert_eq!(
            std::fs::read(root.destination().join(MANIFEST)).expect("original manifest retained"),
            manifest
        );
        assert!(check(&root.destination()).is_ok());
    }
}

#[test]
fn self_consistent_rewrite_cannot_replace_the_prepared_bytes_or_be_cleaned_up() {
    let root = Root::new();
    let prepared = root.prepare();
    let stage = root.stage();
    let mut manifest: Manifest =
        serde_json::from_slice(&std::fs::read(stage.join(MANIFEST)).expect("manifest"))
            .expect("parse manifest");
    let replacement = fixture(true);
    manifest.public_key = header_public_key(replacement).expect("alternate public key");
    manifest.key_sha256 = hex(&Sha256::digest(replacement));
    manifest.key_size = replacement.len() as u64;
    std::fs::write(stage.join(KEY), replacement).expect("rewrite key in place");
    std::fs::write(
        stage.join(MANIFEST),
        serde_json::to_vec(&manifest).expect("encode manifest"),
    )
    .expect("rewrite manifest in place");
    let held = Dir::open_ambient_dir(&stage, cap_std::ambient_authority()).expect("stage handle");
    assert!(
        check_directory(&held).is_ok(),
        "replacement is independently consistent"
    );
    assert_eq!(prepared.publish(), Err(CreateError::Refused));
    assert_eq!(
        std::fs::read(stage.join(KEY)).expect("changed key retained"),
        replacement
    );
    assert!(stage.join(MANIFEST).exists());
    assert!(!root.destination().exists());
}

#[test]
fn replaced_member_added_member_and_changed_permissions_are_preserved() {
    for change in ["replace", "extra", "permissions"] {
        let root = Root::new();
        let prepared = root.prepare();
        let stage = root.stage();
        match change {
            "replace" => {
                std::fs::rename(stage.join(KEY), root.0.join("retained-original"))
                    .expect("retain original object");
                std::fs::write(stage.join(KEY), fixture(false)).expect("replacement object");
                std::fs::set_permissions(stage.join(KEY), std::fs::Permissions::from_mode(0o600))
                    .expect("replacement permissions");
            }
            "extra" => std::fs::write(stage.join("foreign"), b"foreign").expect("foreign member"),
            "permissions" => {
                std::fs::set_permissions(stage.join(KEY), std::fs::Permissions::from_mode(0o640))
                    .expect("changed permissions")
            }
            _ => unreachable!(),
        }
        assert_eq!(prepared.publish(), Err(CreateError::Refused));
        assert!(stage.join(KEY).exists());
        assert!(stage.join(MANIFEST).exists());
        if change == "extra" {
            assert_eq!(
                std::fs::read(stage.join("foreign")).expect("foreign preserved"),
                b"foreign"
            );
        }
        assert!(!root.destination().exists());
    }
}

#[test]
fn destination_claimed_after_preparation_is_never_replaced() {
    let root = Root::new();
    let prepared = root.prepare();
    let stage = root.stage();
    std::fs::create_dir(root.destination()).expect("foreign destination");
    std::fs::write(root.destination().join("foreign"), b"foreign").expect("foreign contents");
    assert_eq!(prepared.publish(), Err(CreateError::Refused));
    assert_eq!(
        std::fs::read(root.destination().join("foreign")).expect("foreign destination preserved"),
        b"foreign"
    );
    assert!(!stage.exists());
}

#[test]
fn destination_replaced_during_final_check_is_committed_indeterminate() {
    let root = Root::new();
    let prepared = root.prepare();
    let retained = root.0.join("published-original");
    let result = prepared.publish_checked(|directory| {
        check_directory(directory)?;
        std::fs::rename(root.destination(), &retained).expect("retain published original");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(root.destination())
            .expect("replacement destination");
        std::fs::write(root.destination().join("foreign"), b"foreign")
            .expect("replacement destination contents");
        Ok(())
    });
    assert_eq!(result, Err(CreateError::CommittedIndeterminate));
    assert_eq!(
        std::fs::read(root.destination().join("foreign")).expect("replacement retained"),
        b"foreign"
    );
    assert_eq!(
        std::fs::read(retained.join(KEY)).expect("published original retained"),
        fixture(false)
    );
    assert!(retained.join(MANIFEST).exists());
    assert!(check(&retained).is_ok());
}

#[test]
fn returned_faults_preserve_precommit_and_committed_error_taxonomy() {
    for (preparation_cut, publication_cut) in [
        (Some(PackagePrepareStage::KeySynced), None),
        (Some(PackagePrepareStage::ManifestSynced), None),
        (None, Some(PackagePublishStage::BeforeRename)),
        (None, Some(PackagePublishStage::Renamed)),
    ] {
        let root = Root::new();
        let result = prepare_observed(
            Location::preflight(&root.destination()).expect("preflight"),
            COMMENT,
            fixture(false),
            |stage| {
                if Some(stage) == preparation_cut {
                    Err(())
                } else {
                    Ok(())
                }
            },
        )
        .and_then(|prepared| {
            prepared.publish_observed(&mut |stage| {
                if Some(stage) == publication_cut {
                    Err(())
                } else {
                    Ok(())
                }
            })
        });
        if publication_cut == Some(PackagePublishStage::Renamed) {
            assert_eq!(result, Err(CreateError::CommittedIndeterminate));
            assert!(check(&root.destination()).is_ok());
        } else {
            assert_eq!(result, Err(CreateError::Refused));
            assert_eq!(
                std::fs::read_dir(&root.0)
                    .expect("root after refusal")
                    .count(),
                0
            );
        }
    }
}
