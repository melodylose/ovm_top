#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Freshness {
    Live,
    Stale,
    Fallback,
    #[default]
    NoData,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Availability {
    Available,
    Empty,
    #[default]
    Unavailable,
    Partial,
    Error,
    CollectorIncompatible,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayerStatus {
    pub freshness: Freshness,
    pub availability: Availability,
    pub reason: Option<String>,
}

impl LayerStatus {
    pub fn live() -> Self {
        Self {
            freshness: Freshness::Live,
            availability: Availability::Available,
            reason: None,
        }
    }

    pub fn empty(reason: impl Into<String>) -> Self {
        Self {
            freshness: Freshness::Live,
            availability: Availability::Empty,
            reason: Some(reason.into()),
        }
    }

    pub fn partial(reason: impl Into<String>) -> Self {
        Self {
            freshness: Freshness::Live,
            availability: Availability::Partial,
            reason: Some(reason.into()),
        }
    }

    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            freshness: Freshness::NoData,
            availability: Availability::Unavailable,
            reason: Some(reason.into()),
        }
    }

    pub fn error(reason: impl Into<String>) -> Self {
        Self {
            freshness: Freshness::NoData,
            availability: Availability::Error,
            reason: Some(reason.into()),
        }
    }

    pub fn fallback(reason: impl Into<String>) -> Self {
        Self {
            freshness: Freshness::Fallback,
            availability: Availability::Available,
            reason: Some(reason.into()),
        }
    }

    pub fn incompatible(reason: impl Into<String>) -> Self {
        Self {
            freshness: Freshness::Fallback,
            availability: Availability::CollectorIncompatible,
            reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LayerStatuses {
    pub inventory: LayerStatus,
    pub realtime: LayerStatus,
    pub vif: LayerStatus,
    pub vbd: LayerStatus,
    pub network_inventory: LayerStatus,
    pub block_inventory: LayerStatus,
    pub fc_hba: LayerStatus,
    pub fc_transport: LayerStatus,
    pub multipath: LayerStatus,
    pub storage_mapping: LayerStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticStage {
    Execute,
    Permission,
    Capability,
    Read,
    Parse,
    Empty,
    Mapping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    CommandMissing,
    PermissionDenied,
    UnsupportedFields,
    EmptyExpected,
    ParseFailure,
    Partial,
    Unavailable,
    CollectorIncompatible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectorDiagnostic {
    pub layer: &'static str,
    pub stage: DiagnosticStage,
    pub kind: DiagnosticKind,
    pub reason: String,
}

pub fn classify_diagnostic(
    layer: &'static str,
    stage: DiagnosticStage,
    reason: impl Into<String>,
) -> CollectorDiagnostic {
    let reason = reason.into();
    let lower = reason.to_ascii_lowercase();
    let (stage, kind) =
        if lower.contains("permission denied") || lower.contains("operation not permitted") {
            (
                DiagnosticStage::Permission,
                DiagnosticKind::PermissionDenied,
            )
        } else if stage == DiagnosticStage::Execute
            && (lower.contains("no such file") || lower.contains("not found"))
        {
            (DiagnosticStage::Execute, DiagnosticKind::CommandMissing)
        } else if lower.contains("unknown column")
            || lower.contains("unsupported")
            || lower.contains("unrecognized option")
        {
            (
                DiagnosticStage::Capability,
                DiagnosticKind::UnsupportedFields,
            )
        } else if lower.contains("parse") || lower.contains("invalid") {
            (DiagnosticStage::Parse, DiagnosticKind::ParseFailure)
        } else {
            (stage, DiagnosticKind::Unavailable)
        };
    CollectorDiagnostic {
        layer,
        stage,
        kind,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_data_can_have_incomplete_mapping_without_becoming_stale() {
        let layers = LayerStatuses {
            inventory: LayerStatus::live(),
            vbd: LayerStatus::live(),
            storage_mapping: LayerStatus::partial("6 VBD mappings unresolved at HostBlock"),
            ..Default::default()
        };

        assert_eq!(layers.vbd.freshness, Freshness::Live);
        assert_eq!(layers.storage_mapping.freshness, Freshness::Live);
        assert_eq!(layers.storage_mapping.availability, Availability::Partial);
    }

    #[test]
    fn partial_collector_failure_does_not_override_other_layers() {
        let layers = LayerStatuses {
            inventory: LayerStatus::live(),
            realtime: LayerStatus::live(),
            vif: LayerStatus::unavailable("xenstore VIF source unavailable"),
            fc_hba: LayerStatus::live(),
            ..Default::default()
        };

        assert_eq!(layers.inventory.availability, Availability::Available);
        assert_eq!(layers.realtime.freshness, Freshness::Live);
        assert_eq!(layers.vif.availability, Availability::Unavailable);
        assert_eq!(layers.fc_hba.availability, Availability::Available);
    }

    #[test]
    fn classifies_common_collector_failures_with_stage_and_reason() {
        assert_eq!(
            classify_diagnostic(
                "block",
                DiagnosticStage::Execute,
                "lsblk: command not found"
            )
            .kind,
            DiagnosticKind::CommandMissing
        );
        assert_eq!(
            classify_diagnostic("fc", DiagnosticStage::Read, "Permission denied").kind,
            DiagnosticKind::PermissionDenied
        );
        assert_eq!(
            classify_diagnostic(
                "block",
                DiagnosticStage::Capability,
                "lsblk: unknown column WWN"
            )
            .kind,
            DiagnosticKind::UnsupportedFields
        );
        assert_eq!(
            classify_diagnostic("multipath", DiagnosticStage::Parse, "invalid map header").kind,
            DiagnosticKind::ParseFailure
        );
    }

    #[test]
    fn collector_incompatible_can_still_have_fresh_fallback_data() {
        let status = LayerStatus::incompatible("old lsblk columns; sysfs fallback succeeded");

        assert_eq!(status.freshness, Freshness::Fallback);
        assert_eq!(status.availability, Availability::CollectorIncompatible);
    }
}
