#[cfg(test)]
mod vivac_format_review {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    // Public fixtures only. Passing these observations does not approve custody.
    const PUBLIC_PASSWORD: &str = "public oo7 format review fixture";
    const UNRELATED_PASSWORD: &str = "different public review fixture";
    const EXPECTED_ATTRIBUTES: [(&str, &str); 1] = [("fixture", "expected-item")];

    #[test]
    fn empty_container_accepts_two_unrelated_passwords() -> Result<(), Error> {
        let keyring = Keyring::new()?;

        assert!(keyring.validate_secret(&Secret::text(PUBLIC_PASSWORD))?);
        assert!(keyring.validate_secret(&Secret::text(UNRELATED_PASSWORD))?);

        Ok(())
    }

    #[test]
    fn removing_all_items_preserves_password_success_without_expected_item() -> Result<(), Error> {
        let mut keyring = Keyring::new()?;
        let password = Secret::text(PUBLIC_PASSWORD);
        let unrelated = Secret::text(UNRELATED_PASSWORD);
        let key = keyring.derive_key(&password)?;
        let item = UnlockedItem::new(
            "Public format review fixture",
            &EXPECTED_ATTRIBUTES,
            Secret::blob(b"public item payload"),
        );
        keyring.items.push(item.encrypt(Some(&key))?);

        assert!(keyring.validate_secret(&password)?);
        assert!(!keyring.validate_secret(&unrelated)?);
        assert_eq!(
            keyring.search_items(&EXPECTED_ATTRIBUTES, Some(&key))?.len(),
            1
        );

        keyring.items.clear();
        let serialized = keyring.as_bytes()?;
        let parsed = Keyring::try_from(serialized.as_slice())?;

        assert!(parsed.validate_secret(&password)?);
        assert!(parsed.validate_secret(&unrelated)?);
        assert!(
            parsed
                .search_items(&EXPECTED_ATTRIBUTES, Some(&key))?
                .is_empty()
        );

        Ok(())
    }

    #[test]
    fn deleting_external_attribute_hashes_preserves_authenticated_item() -> Result<(), Error> {
        let keyring = Keyring::new()?;
        let key = keyring.derive_key(&Secret::text(PUBLIC_PASSWORD))?;
        let original = UnlockedItem::new(
            "Public format review fixture",
            &EXPECTED_ATTRIBUTES,
            Secret::blob(b"public item payload"),
        );
        let mut encrypted = original.encrypt(Some(&key))?;
        assert!(!encrypted.hashed_attributes.is_empty());
        let authenticated_blob = encrypted.blob.clone();
        assert_eq!(encrypted.clone().decrypt(Some(&key))?, original);

        encrypted.hashed_attributes.clear();

        assert_eq!(encrypted.blob, authenticated_blob);
        assert_eq!(encrypted.decrypt(Some(&key))?, original);

        Ok(())
    }

    #[test]
    fn encrypted_blobs_shorter_than_mac_panic_instead_of_returning_error() {
        let public_key = Key::new(vec![7; 16]);

        for length in 0..32 {
            let item = EncryptedItem {
                hashed_attributes: Default::default(),
                blob: vec![0; length],
            };
            let observed = catch_unwind(AssertUnwindSafe(|| item.decrypt(Some(&public_key))));

            assert!(observed.is_err(), "length {length} did not panic");
        }
    }

    #[test]
    fn maximum_iteration_count_passes_strength_check_without_derivation() -> Result<(), Error> {
        let mut keyring = Keyring::new()?;
        keyring.iteration_count = u32::MAX;

        // Do not derive: the unbounded counter is the observation being tested.
        assert!(keyring.key_strength().is_ok());

        Ok(())
    }
}
