use crate::BoxError;
use secret_service::EncryptionType;
use secret_service::blocking::{Collection, Item, SecretService};
use std::collections::HashMap;
use std::path::Path;
use zeroize::Zeroizing;

const ACCOUNT: &str = "fixture-device";
const CONTENT_TYPE: &str = "application/octet-stream";

fn attributes(account: &str) -> HashMap<&str, &str> {
    HashMap::from([
        ("application", "vivac-native-key-store-probe"),
        ("account", account),
    ])
}

fn exact_item<'a>(collection: &'a Collection<'_>, account: &str) -> Result<Item<'a>, BoxError> {
    let mut items = collection.search_items(attributes(account))?;
    if items.len() != 1 {
        return Err("expected exactly one fixture item".into());
    }
    let item = items.remove(0);
    let actual = item.get_attributes()?;
    let expected: HashMap<String, String> = attributes(account)
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    if actual != expected {
        return Err("fixture attributes do not match".into());
    }
    Ok(item)
}

fn read(root: &Path, collection: &Collection<'_>) -> Result<(), BoxError> {
    if collection.is_locked()? {
        return Err("fixture collection is locked".into());
    }
    let item = exact_item(collection, ACCOUNT)?;
    if item.is_locked()? || item.get_secret_content_type()? != CONTENT_TYPE {
        return Err("fixture item is locked or has an invalid content type".into());
    }
    let payload = Zeroizing::new(item.get_secret()?);
    if payload.len() > 4096 {
        return Err("fixture payload exceeds its size limit".into());
    }
    crate::verify_payload(root, &payload)
}

fn missing_provider(error: &secret_service::Error) -> bool {
    match error {
        secret_service::Error::Zbus(zbus::Error::MethodError(name, ..)) => {
            name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown"
        }
        secret_service::Error::Zbus(zbus::Error::FDO(error)) => {
            matches!(error.as_ref(), zbus::fdo::Error::ServiceUnknown(_))
        }
        secret_service::Error::ZbusFdo(zbus::fdo::Error::ServiceUnknown(_)) => true,
        _ => false,
    }
}

pub fn run(root: &Path, stage: &str) -> Result<(), BoxError> {
    if stage == "missing" {
        return match SecretService::connect(EncryptionType::Dh) {
            Err(error) if missing_provider(&error) => Ok(()),
            Err(_) => Err("provider absence was not confirmed by the session bus".into()),
            Ok(_) => Err("unexpected Secret Service provider".into()),
        };
    }
    let service = SecretService::connect(EncryptionType::Dh)?;
    let collection = service.get_default_collection()?;
    if collection.is_locked()? {
        return Err("fixture collection is locked; prompts are forbidden".into());
    }
    match stage {
        "ready" => Ok(()),
        "create" => {
            if !collection.search_items(attributes(ACCOUNT))?.is_empty() {
                return Err("fixture already exists".into());
            }
            let payload = crate::payload()?;
            collection.create_item(
                "Vivac native key store fixture",
                attributes(ACCOUNT),
                &payload,
                false,
                CONTENT_TYPE,
            )?;
            crate::save_public(root, &payload)?;
            read(root, &collection)
        }
        "read" => read(root, &collection),
        "wrong" => {
            read(root, &collection)?;
            if !collection
                .search_items(attributes("absent-fixture-device"))?
                .is_empty()
            {
                return Err("unexpected item for the wrong account".into());
            }
            Ok(())
        }
        "locked" => {
            let item = exact_item(&collection, ACCOUNT)?;
            collection.lock()?;
            if !collection.is_locked()? || item.get_secret().is_ok() {
                return Err("locked fixture allowed secret retrieval".into());
            }
            if read(root, &collection).is_ok() {
                return Err("locked fixture was accepted".into());
            }
            Ok(())
        }
        "corrupt" => {
            let item = exact_item(&collection, ACCOUNT)?;
            item.set_secret(&[0; 31], CONTENT_TYPE)?;
            let damaged = Zeroizing::new(item.get_secret()?);
            if damaged.as_slice() != [0; 31] || crate::verify_payload(root, &damaged).is_ok() {
                return Err("corrupt fixture was not rejected by payload validation".into());
            }
            if read(root, &collection).is_ok() {
                return Err("corrupt fixture was accepted".into());
            }
            Ok(())
        }
        "ambiguous" => {
            let payload = crate::payload()?;
            collection.create_item(
                "Vivac duplicate fixture",
                attributes(ACCOUNT),
                &payload,
                false,
                CONTENT_TYPE,
            )?;
            if collection.search_items(attributes(ACCOUNT))?.len() != 2 {
                return Err("provider did not retain duplicate fixtures".into());
            }
            if exact_item(&collection, ACCOUNT).is_ok() || read(root, &collection).is_ok() {
                return Err("ambiguous fixture was accepted".into());
            }
            Ok(())
        }
        _ => Err("unknown Linux probe stage".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::missing_provider;

    #[test]
    fn only_service_unknown_confirms_provider_absence() {
        use secret_service::Error;
        use zbus::fdo;

        assert!(missing_provider(&Error::ZbusFdo(
            fdo::Error::ServiceUnknown(String::new())
        )));
        assert!(missing_provider(&Error::Zbus(zbus::Error::FDO(Box::new(
            fdo::Error::ServiceUnknown(String::new())
        )))));
        for error in [
            Error::Unavailable,
            Error::Crypto("fixture"),
            Error::ZbusFdo(fdo::Error::NoReply(String::new())),
            Error::ZbusFdo(fdo::Error::TimedOut(String::new())),
            Error::Zbus(zbus::Error::FDO(Box::new(fdo::Error::AccessDenied(
                String::new(),
            )))),
        ] {
            assert!(!missing_provider(&error));
        }
    }
}
