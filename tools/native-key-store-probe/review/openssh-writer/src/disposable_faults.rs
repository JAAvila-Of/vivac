use crate::package::{
    self, CreateError, Location, PackagePrepareStage, PackagePublishStage, PreparedPackage,
};

type Result<T> = std::result::Result<T, ()>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FaultPoint {
    None,
    KeySynced,
    ManifestSynced,
    BeforePublish,
    AfterPublish,
    ReturnKeySynced,
    ReturnManifestSynced,
    ReturnBeforePublish,
    ReturnAfterPublish,
}

impl FaultPoint {
    pub(crate) fn from_environment() -> Result<Self> {
        let read = |name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(()),
        };
        let exit = read("VIVAC_OPENSSH_PACKAGE_EXIT_AT")?;
        let returned = read("VIVAC_OPENSSH_PACKAGE_ERROR_AT")?;
        Self::parse(exit.as_deref(), returned.as_deref())
    }

    fn parse(exit: Option<&str>, returned: Option<&str>) -> Result<Self> {
        let exit = exit.filter(|value| !value.is_empty());
        let returned = returned.filter(|value| !value.is_empty());
        if exit.is_some() && returned.is_some() {
            return Err(());
        }
        match (exit, returned) {
            (None, None) => Ok(Self::None),
            (Some("key-synced"), None) => Ok(Self::KeySynced),
            (Some("manifest-synced"), None) => Ok(Self::ManifestSynced),
            (Some("before-publish"), None) => Ok(Self::BeforePublish),
            (Some("after-publish"), None) => Ok(Self::AfterPublish),
            (None, Some("key-synced")) => Ok(Self::ReturnKeySynced),
            (None, Some("manifest-synced")) => Ok(Self::ReturnManifestSynced),
            (None, Some("before-publish")) => Ok(Self::ReturnBeforePublish),
            (None, Some("after-publish")) => Ok(Self::ReturnAfterPublish),
            _ => Err(()),
        }
    }

    fn hit(self, point: Self) -> Result<()> {
        if self == point {
            std::process::exit(77);
        }
        match (self, point) {
            (Self::ReturnKeySynced, Self::KeySynced)
            | (Self::ReturnManifestSynced, Self::ManifestSynced)
            | (Self::ReturnBeforePublish, Self::BeforePublish)
            | (Self::ReturnAfterPublish, Self::AfterPublish) => Err(()),
            _ => Ok(()),
        }
    }

    pub(crate) fn observe_publication(self, stage: PackagePublishStage) -> Result<()> {
        match stage {
            PackagePublishStage::BeforeRename => self.hit(Self::BeforePublish),
            PackagePublishStage::Renamed => self.hit(Self::AfterPublish),
            PackagePublishStage::ParentSynced => Ok(()),
        }
    }

    pub(crate) fn observe_preparation(self, stage: PackagePrepareStage) -> Result<()> {
        self.hit(match stage {
            PackagePrepareStage::KeySynced => Self::KeySynced,
            PackagePrepareStage::ManifestSynced => Self::ManifestSynced,
        })
    }
}

pub(crate) fn publish(
    prepared: PreparedPackage,
    fault: FaultPoint,
) -> std::result::Result<(), CreateError> {
    if fault == FaultPoint::None {
        prepared.publish()
    } else {
        prepared.publish_observed(&mut |stage| fault.observe_publication(stage))
    }
}

pub(crate) fn create(
    location: Location,
    comment: &str,
    encoded: &[u8],
    fault: FaultPoint,
) -> std::result::Result<(), CreateError> {
    let prepared = package::prepare_observed(location, comment, encoded, |stage| {
        fault.observe_preparation(stage)
    })?;
    publish(prepared, fault)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_fault_channels_are_exclusive_and_strict() {
        assert_eq!(FaultPoint::parse(None, None), Ok(FaultPoint::None));
        assert_eq!(FaultPoint::parse(Some(""), Some("")), Ok(FaultPoint::None));
        for (name, exit, returned) in [
            (
                "key-synced",
                FaultPoint::KeySynced,
                FaultPoint::ReturnKeySynced,
            ),
            (
                "manifest-synced",
                FaultPoint::ManifestSynced,
                FaultPoint::ReturnManifestSynced,
            ),
            (
                "before-publish",
                FaultPoint::BeforePublish,
                FaultPoint::ReturnBeforePublish,
            ),
            (
                "after-publish",
                FaultPoint::AfterPublish,
                FaultPoint::ReturnAfterPublish,
            ),
        ] {
            assert_eq!(FaultPoint::parse(Some(name), None), Ok(exit));
            assert_eq!(FaultPoint::parse(None, Some(name)), Ok(returned));
            assert_eq!(FaultPoint::parse(Some(""), Some(name)), Ok(returned));
            assert!(FaultPoint::parse(Some(name), Some(name)).is_err());
        }
        for invalid in ["unknown", " key-synced", "key-synced ", "KEY-SYNCED"] {
            assert!(FaultPoint::parse(Some(invalid), None).is_err());
            assert!(FaultPoint::parse(None, Some(invalid)).is_err());
        }
    }

    #[test]
    fn injected_errors_return_only_at_the_selected_publication_seam() {
        let seams = [
            FaultPoint::KeySynced,
            FaultPoint::ManifestSynced,
            FaultPoint::BeforePublish,
            FaultPoint::AfterPublish,
        ];
        let returned = [
            FaultPoint::ReturnKeySynced,
            FaultPoint::ReturnManifestSynced,
            FaultPoint::ReturnBeforePublish,
            FaultPoint::ReturnAfterPublish,
        ];
        for (selected, fault) in returned.into_iter().enumerate() {
            for (position, seam) in seams.into_iter().enumerate() {
                assert_eq!(fault.hit(seam).is_err(), selected == position);
            }
            assert_eq!(
                fault
                    .observe_preparation(PackagePrepareStage::KeySynced)
                    .is_err(),
                selected == 0
            );
            assert_eq!(
                fault
                    .observe_preparation(PackagePrepareStage::ManifestSynced)
                    .is_err(),
                selected == 1
            );
            assert_eq!(
                fault
                    .observe_publication(PackagePublishStage::BeforeRename)
                    .is_err(),
                selected == 2
            );
            assert_eq!(
                fault
                    .observe_publication(PackagePublishStage::Renamed)
                    .is_err(),
                selected == 3
            );
            assert!(fault
                .observe_publication(PackagePublishStage::ParentSynced)
                .is_ok());
        }
        for seam in seams {
            assert!(FaultPoint::None.hit(seam).is_ok());
        }
    }
}
