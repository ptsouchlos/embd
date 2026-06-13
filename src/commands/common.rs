use anyhow::{Context, Result};

use crate::config::{Config, EmbdEntry};

/// Resolve a list of user-supplied names to `(name, entry)` pairs. An empty
/// `names` slice means "all entries, in deterministic order."
pub(crate) fn select_entries<'a>(
    config: &'a Config,
    names: &'a [String],
) -> Result<Vec<(&'a str, &'a EmbdEntry)>> {
    if names.is_empty() {
        return Ok(config.iter().collect());
    }
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let entry = config
            .get(name)
            .with_context(|| format!("no embed named '{name}' in config"))?;
        out.push((name.as_str(), entry));
    }
    Ok(out)
}
