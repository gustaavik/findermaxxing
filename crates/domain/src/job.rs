use crate::path::PathSegment;
use crate::units::ByteSize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobPhase {
    /// Walking the sources to learn item count and total size.
    Preparing,
    Transferring,
}

/// Snapshot of a running job's progress. Totals are known once preparing ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobProgress {
    pub phase: JobPhase,
    pub items_done: u64,
    pub items_total: u64,
    pub bytes_done: ByteSize,
    pub bytes_total: ByteSize,
    pub current_item: Option<PathSegment>,
}

impl JobProgress {
    #[must_use]
    pub fn preparing() -> Self {
        Self {
            phase: JobPhase::Preparing,
            items_done: 0,
            items_total: 0,
            bytes_done: ByteSize(0),
            bytes_total: ByteSize(0),
            current_item: None,
        }
    }

    /// Completed share in `0.0..=1.0`, by bytes when there are any, else by items.
    /// `None` while the totals are still unknown.
    #[must_use]
    #[allow(clippy::cast_precision_loss, reason = "a progress bar needs no 53-bit precision")]
    pub fn fraction(&self) -> Option<f64> {
        if self.phase == JobPhase::Preparing {
            return None;
        }
        let (done, total) = if self.bytes_total.0 > 0 {
            (self.bytes_done.0, self.bytes_total.0)
        } else {
            (self.items_done, self.items_total)
        };
        Some(if total == 0 { 1.0 } else { (done as f64 / total as f64).min(1.0) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_prefers_bytes_and_is_unknown_while_preparing() {
        let mut progress = JobProgress::preparing();
        assert_eq!(progress.fraction(), None);
        progress.phase = JobPhase::Transferring;
        progress.items_total = 4;
        progress.items_done = 1;
        assert_eq!(progress.fraction(), Some(0.25));
        progress.bytes_total = ByteSize(200);
        progress.bytes_done = ByteSize(150);
        assert_eq!(progress.fraction(), Some(0.75));
    }
}
