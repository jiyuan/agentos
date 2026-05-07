use agentos_interfaces::memory::{Query, Record, Selector};
use agentos_interfaces::orchestrator::MemoryFragment;
use serde_json::Value;

pub(crate) fn record_matches_query(record: &Record, q: &Query) -> bool {
    if !record_is_active(record) {
        return false;
    }
    let Some(query) = q.lexical_text().map(str::trim) else {
        return true;
    };
    if query.is_empty() {
        return true;
    }
    let needle = query.to_ascii_lowercase();
    record
        .body
        .to_string()
        .to_ascii_lowercase()
        .contains(&needle)
        || serde_json::to_string(&record.metadata)
            .map(|metadata| metadata.to_ascii_lowercase().contains(&needle))
            .unwrap_or(false)
}

pub(crate) fn record_is_active(record: &Record) -> bool {
    record
        .metadata
        .get("status")
        .and_then(Value::as_str)
        .map(|status| status == "active")
        .unwrap_or(true)
}

pub(super) fn selector_matches_record(record: &Record, selector: &Selector) -> bool {
    if let Some(id) = &selector.id {
        return record.id.as_ref() == Some(id);
    }
    if let Some(namespace) = &selector.namespace {
        return &record.namespace == namespace;
    }
    true
}

pub(super) fn estimate_fragment_tokens(fragment: &MemoryFragment) -> usize {
    let body_chars = fragment.body.to_string().chars().count();
    let metadata_chars = serde_json::to_string(&fragment.metadata)
        .map(|metadata| metadata.chars().count())
        .unwrap_or(0);
    (body_chars + metadata_chars).div_ceil(4).max(1)
}
