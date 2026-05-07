use std::sync::Arc;

pub(super) fn normalize_config_token(input: &str) -> String {
    input.trim().to_ascii_lowercase().replace('-', "_")
}

pub(super) fn normalize_domain(input: &str, name: &str) -> Result<Arc<str>, String> {
    let normalized = input
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let normalized = normalized.trim_matches('_');
    if normalized.is_empty() {
        Err(format!("{name} must not be empty"))
    } else {
        Ok(Arc::from(normalized))
    }
}
