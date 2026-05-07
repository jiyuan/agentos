use super::{MemoryCaller, MemoryError};
use agentos_proto::{Namespace, RecordId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MemoryOperation {
    Read,
    Write,
    Forget,
}

impl MemoryOperation {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Forget => "forget",
        }
    }
}

pub(super) trait MemoryAccounting: Send + Sync {
    fn record_read_access(&self, record_ids: &[RecordId]) -> Result<(), MemoryError>;
    fn append_access_log(&self, entry: MemoryAccessLogEntry<'_>) -> Result<(), MemoryError>;

    fn append_access_log_for_records(
        &self,
        operation: &'static str,
        record_ids: &[RecordId],
        namespace: &Namespace,
        caller: &MemoryCaller,
        reason: Option<&str>,
    ) -> Result<(), MemoryError> {
        for record_id in record_ids {
            self.append_access_log(MemoryAccessLogEntry {
                operation,
                record_id: Some(record_id),
                namespace,
                caller,
                reason,
            })?;
        }
        Ok(())
    }
}

pub(super) struct MemoryAccessLogEntry<'a> {
    pub(super) operation: &'static str,
    pub(super) record_id: Option<&'a RecordId>,
    pub(super) namespace: &'a Namespace,
    pub(super) caller: &'a MemoryCaller,
    pub(super) reason: Option<&'a str>,
}
