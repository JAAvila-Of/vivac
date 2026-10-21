use serde::{Deserialize, Serialize};

use crate::failure::Failure;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapturePolicy {
    Strict,
    AllowInsecure,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Detected,
    NotDetected,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Inspection {
    pub policy: CapturePolicy,
    pub sensitivity: Sensitivity,
    pub coverage: Coverage,
    pub review_required: bool,
}

impl Inspection {
    pub(crate) fn strict() -> Self {
        Self {
            policy: CapturePolicy::Strict,
            sensitivity: Sensitivity::NotDetected,
            coverage: Coverage::Complete,
            review_required: false,
        }
    }

    pub(crate) fn permissive() -> Self {
        Self {
            policy: CapturePolicy::AllowInsecure,
            review_required: true,
            ..Self::strict()
        }
    }

    pub(crate) fn merge(&mut self, other: Self) {
        if other.policy == CapturePolicy::AllowInsecure {
            self.policy = CapturePolicy::AllowInsecure;
        }
        self.review_required |= other.review_required;
        self.coverage = match (self.coverage, other.coverage) {
            (Coverage::Unavailable, _) | (_, Coverage::Unavailable) => Coverage::Unavailable,
            (Coverage::Partial, _) | (_, Coverage::Partial) => Coverage::Partial,
            _ => Coverage::Complete,
        };
        self.sensitivity = match (self.sensitivity, other.sensitivity) {
            (Sensitivity::Detected, _) | (_, Sensitivity::Detected) => Sensitivity::Detected,
            (Sensitivity::Unknown, _) | (_, Sensitivity::Unknown) => Sensitivity::Unknown,
            _ if self.coverage != Coverage::Complete => Sensitivity::Unknown,
            _ => Sensitivity::NotDetected,
        };
    }

    pub(crate) fn validate(&self) -> Result<(), Failure> {
        let valid = match self.policy {
            CapturePolicy::Strict => {
                self.coverage == Coverage::Complete && self.sensitivity == Sensitivity::NotDetected
            }
            CapturePolicy::AllowInsecure => {
                self.review_required
                    && match self.sensitivity {
                        Sensitivity::Detected => true,
                        Sensitivity::NotDetected => self.coverage == Coverage::Complete,
                        Sensitivity::Unknown => self.coverage != Coverage::Complete,
                    }
            }
        };
        if valid {
            Ok(())
        } else {
            Err(Failure::usage("Invalid artifact inspection status."))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_merge_preserves_detected_risk_and_incomplete_coverage() {
        let mut inspection = Inspection::strict();
        inspection.merge(Inspection {
            sensitivity: Sensitivity::Detected,
            ..Inspection::permissive()
        });
        inspection.merge(Inspection {
            sensitivity: Sensitivity::Unknown,
            coverage: Coverage::Unavailable,
            ..Inspection::permissive()
        });
        inspection.merge(Inspection::strict());
        assert_eq!(inspection.policy, CapturePolicy::AllowInsecure);
        assert_eq!(inspection.sensitivity, Sensitivity::Detected);
        assert_eq!(inspection.coverage, Coverage::Unavailable);
        assert!(inspection.review_required);
        inspection.validate().unwrap();

        let mut incomplete = Inspection::permissive();
        incomplete.merge(Inspection {
            coverage: Coverage::Partial,
            ..Inspection::permissive()
        });
        assert_eq!(incomplete.sensitivity, Sensitivity::Unknown);
        incomplete.validate().unwrap();
    }

    #[test]
    fn inspection_validation_refuses_contradictory_or_unreviewed_risk() {
        Inspection::strict().validate().unwrap();
        Inspection::permissive().validate().unwrap();
        Inspection {
            review_required: true,
            ..Inspection::strict()
        }
        .validate()
        .unwrap();
        for invalid in [
            Inspection {
                sensitivity: Sensitivity::Detected,
                ..Inspection::strict()
            },
            Inspection {
                sensitivity: Sensitivity::Unknown,
                coverage: Coverage::Partial,
                ..Inspection::strict()
            },
            Inspection {
                review_required: false,
                ..Inspection::permissive()
            },
            Inspection {
                sensitivity: Sensitivity::Unknown,
                ..Inspection::permissive()
            },
            Inspection {
                coverage: Coverage::Partial,
                ..Inspection::permissive()
            },
            Inspection {
                coverage: Coverage::Unavailable,
                ..Inspection::permissive()
            },
        ] {
            assert_eq!(invalid.validate().unwrap_err().code(), 2);
        }
    }

    #[test]
    fn inspection_json_roundtrip_refuses_unknown_fields_and_values() {
        let inspection = Inspection::permissive();
        let encoded = serde_json::to_value(inspection).unwrap();
        assert_eq!(encoded["policy"], "allow_insecure");
        assert_eq!(encoded["sensitivity"], "not_detected");
        assert_eq!(encoded["coverage"], "complete");
        assert_eq!(
            serde_json::from_value::<Inspection>(encoded.clone()).unwrap(),
            inspection
        );
        let mut extra = encoded.clone();
        extra["content"] = serde_json::json!("not retained");
        assert!(serde_json::from_value::<Inspection>(extra).is_err());
        for field in ["policy", "sensitivity", "coverage"] {
            let mut unknown = encoded.clone();
            unknown[field] = serde_json::json!("unsupported");
            assert!(serde_json::from_value::<Inspection>(unknown).is_err());
        }
    }
}
