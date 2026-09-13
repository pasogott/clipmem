use crate::db::types::OcrRunReport;

impl OcrRunReport {
    #[must_use]
    pub(crate) fn new(
        processed: usize,
        ready: usize,
        failed: usize,
        skipped: usize,
        remaining_pending: usize,
    ) -> Self {
        Self {
            processed,
            ready,
            failed,
            skipped,
            remaining_pending,
        }
    }

    #[must_use]
    pub(crate) fn processed(&self) -> usize {
        self.processed
    }

    #[must_use]
    pub(crate) fn ready(&self) -> usize {
        self.ready
    }

    #[must_use]
    pub(crate) fn failed(&self) -> usize {
        self.failed
    }

    #[must_use]
    pub(crate) fn skipped(&self) -> usize {
        self.skipped
    }

    #[must_use]
    pub(crate) fn remaining_pending(&self) -> usize {
        self.remaining_pending
    }
}
