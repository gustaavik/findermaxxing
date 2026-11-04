//! When a transfer may cost the user money, and why.

use crate::units::ByteSize;

/// Default threshold for "large transfer" warnings: 1 GB.
pub const DEFAULT_LARGE_TRANSFER: ByteSize = ByteSize(1_000_000_000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostPolicy {
    pub large_transfer: ByteSize,
}

impl Default for CostPolicy {
    fn default() -> Self {
        Self { large_transfer: DEFAULT_LARGE_TRANSFER }
    }
}

/// What a planned transfer involves, as far as billing is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferEstimate {
    pub bytes: ByteSize,
    pub items: u64,
    /// The transfer reads from or writes to a metered (billed) provider.
    pub metered: bool,
    /// Regions of the metered source and destination, when known.
    pub source_region: Option<String>,
    pub destination_region: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CostReason {
    LargeTransfer(ByteSize),
    CrossRegion { from: String, to: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostWarning {
    pub reasons: Vec<CostReason>,
    pub bytes: ByteSize,
    pub items: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostDecision {
    Proceed,
    /// Proceed, and stop asking for the accounts involved.
    ProceedAndStopAsking,
    Cancel,
}

impl CostPolicy {
    /// `None` when the transfer needs no confirmation.
    #[must_use]
    pub fn assess(&self, estimate: &TransferEstimate) -> Option<CostWarning> {
        if !estimate.metered {
            return None;
        }
        let mut reasons = Vec::new();
        if estimate.bytes >= self.large_transfer {
            reasons.push(CostReason::LargeTransfer(estimate.bytes));
        }
        if let (Some(from), Some(to)) = (&estimate.source_region, &estimate.destination_region)
            && from != to
        {
            reasons.push(CostReason::CrossRegion { from: from.clone(), to: to.clone() });
        }
        (!reasons.is_empty()).then_some(CostWarning { reasons, bytes: estimate.bytes, items: estimate.items })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn estimate(bytes: u64, metered: bool, from: Option<&str>, to: Option<&str>) -> TransferEstimate {
        TransferEstimate {
            bytes: ByteSize(bytes),
            items: 1,
            metered,
            source_region: from.map(str::to_owned),
            destination_region: to.map(str::to_owned),
        }
    }

    #[test]
    fn only_metered_transfers_are_assessed() {
        assert_eq!(CostPolicy::default().assess(&estimate(5_000_000_000, false, None, None)), None);
    }

    #[test]
    fn large_and_cross_region_transfers_warn() {
        let policy = CostPolicy::default();
        assert_eq!(policy.assess(&estimate(10, true, Some("eu-west-1"), Some("eu-west-1"))), None);
        let warning = policy.assess(&estimate(2_000_000_000, true, Some("eu-west-1"), Some("us-east-1"))).unwrap();
        assert_eq!(
            warning.reasons,
            vec![
                CostReason::LargeTransfer(ByteSize(2_000_000_000)),
                CostReason::CrossRegion { from: "eu-west-1".into(), to: "us-east-1".into() },
            ]
        );
    }
}
