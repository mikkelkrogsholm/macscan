//! Supplemental metadata only; no sharing or reclaimability accounting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CloneStatus {
    #[default]
    NotRequested,
    Observed,
    Missing,
    Unsupported,
    Invalid,
}
impl CloneStatus {
    pub fn name(self) -> &'static str {
        match self {
            Self::NotRequested => "not_requested",
            Self::Observed => "observed",
            Self::Missing => "missing",
            Self::Unsupported => "unsupported",
            Self::Invalid => "invalid",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CloneMetadata {
    pub status: CloneStatus,
    pub clone_id: Option<u64>,
    pub extended_flags: Option<u64>,
    pub data_allocated: Option<u64>,
    /// Device from the same returned metadata record, never filled from fallback stat.
    pub device: Option<u64>,
}
impl CloneMetadata {
    pub fn unavailable(status: CloneStatus) -> Self {
        Self {
            status,
            ..Self::default()
        }
    }
}
pub const CLONE_CAPTURE_VERSION: u32 = 1;
impl CloneMetadata {
    pub(crate) fn json(self) -> serde_json::Value {
        serde_json::json!({"status":self.status.name(),"clone_id":self.clone_id.map(|n|n.to_string()),"extended_flags":self.extended_flags.map(|n|n.to_string()),"data_allocated":self.data_allocated.map(|n|n.to_string()),"device":self.device.map(|n|n.to_string())})
    }
    pub(crate) fn from_json(v: &serde_json::Value) -> std::io::Result<Self> {
        let bad = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid supplemental clone metadata",
            )
        };
        let status = match v["status"].as_str() {
            Some("not_requested") => CloneStatus::NotRequested,
            Some("observed") => CloneStatus::Observed,
            Some("missing") => CloneStatus::Missing,
            Some("unsupported") => CloneStatus::Unsupported,
            Some("invalid") => CloneStatus::Invalid,
            _ => return Err(bad()),
        };
        let field = |name: &str| -> std::io::Result<Option<u64>> {
            if v[name].is_null() {
                Ok(None)
            } else {
                v[name]
                    .as_str()
                    .and_then(|s| s.parse().ok())
                    .map(Some)
                    .ok_or_else(bad)
            }
        };
        let result = Self {
            status,
            clone_id: field("clone_id")?,
            extended_flags: field("extended_flags")?,
            data_allocated: field("data_allocated")?,
            device: field("device")?,
        };
        if status == CloneStatus::Observed
            && (result.clone_id.is_none()
                || result.extended_flags.is_none()
                || result.data_allocated.is_none()
                || result.device.is_none())
        {
            return Err(bad());
        }
        Ok(result)
    }
}
