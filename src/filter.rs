//! Include/exclude glob filtering for embedded folders. A single [`Filter`] is
//! the source of truth for "should this file be pulled?", consulted by both the
//! copy walker (`filesystem::copy_dir`) and the manifest walker
//! (`cache::build_from_path_filtered`) so `add` and `update` stay in agreement.
//!
//! Semantics: a file is pulled when it matches the include set
//! (or the include set is empty, meaning "everything") AND it does not match the
//! exclude set. Excludes therefore win over includes.
//!
//! Patterns are matched against forward-slash relative paths (the same keys used
//! by [`crate::cache::path_to_key`]). Globs are compiled with
//! `literal_separator(true)`, so `*` does not cross `/`; use `**` for recursive
//! matches (e.g. `**/*.rs`, `docs/**`).

use anyhow::{Context, Result};
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

/// Decides whether a given relative path should be pulled into an embed.
#[derive(Debug)]
pub(crate) struct Filter {
    /// `None` means "include everything" (no include patterns were given).
    include: Option<GlobSet>,
    /// `None` means "exclude nothing" (no exclude patterns were given).
    exclude: Option<GlobSet>,
}

impl Filter {
    /// Compile a filter from raw pattern strings (as stored in the config or
    /// passed on the CLI). An empty slice yields no constraint for that side.
    ///
    /// # Errors
    /// Returns an error naming the offending pattern if any glob fails to compile.
    pub(crate) fn from_patterns(include: &[String], exclude: &[String]) -> Result<Self> {
        Ok(Self {
            include: build_set(include)?,
            exclude: build_set(exclude)?,
        })
    }

    /// An all-pass filter: includes everything, excludes nothing.
    pub(crate) fn allow_all() -> Self {
        Self {
            include: None,
            exclude: None,
        }
    }

    /// Return true if a file at the given forward-slash relative `key` should be
    /// included.
    pub(crate) fn includes(&self, key: &str) -> bool {
        let included = match &self.include {
            Some(set) => set.is_match(key),
            None => true,
        };
        if !included {
            return false;
        }
        match &self.exclude {
            Some(set) => !set.is_match(key),
            None => true,
        }
    }
}

/// Compile a list of patterns into a [`GlobSet`], or `None` when the list is empty.
fn build_set(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(compile(pattern)?);
    }
    let set = builder
        .build()
        .context("failed to build glob set from patterns")?;
    Ok(Some(set))
}

/// Compile a single glob with separator-aware matching.
fn compile(pattern: &str) -> Result<Glob> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .with_context(|| format!("invalid glob pattern '{pattern}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(include: &[&str], exclude: &[&str]) -> Filter {
        let inc: Vec<String> = include.iter().map(|s| s.to_string()).collect();
        let exc: Vec<String> = exclude.iter().map(|s| s.to_string()).collect();
        Filter::from_patterns(&inc, &exc).unwrap()
    }

    #[test]
    fn empty_filter_includes_everything() {
        let f = Filter::allow_all();
        assert!(f.includes("a.txt"));
        assert!(f.includes("deep/nested/path.rs"));
    }

    #[test]
    fn from_empty_patterns_includes_everything() {
        let f = filter(&[], &[]);
        assert!(f.includes("anything"));
    }

    #[test]
    fn include_only_acts_as_allowlist() {
        let f = filter(&["src/**"], &[]);
        assert!(f.includes("src/main.rs"));
        assert!(f.includes("src/a/b.rs"));
        assert!(!f.includes("docs/readme.md"));
        assert!(!f.includes("Cargo.toml"));
    }

    #[test]
    fn exclude_only_acts_as_denylist() {
        let f = filter(&[], &["**/*.md", "docs/**"]);
        assert!(f.includes("src/main.rs"));
        assert!(!f.includes("readme.md"));
        assert!(!f.includes("src/notes.md"));
        assert!(!f.includes("docs/guide/intro.txt"));
    }

    #[test]
    fn exclude_beats_include() {
        let f = filter(&["src/**"], &["**/*.test.rs"]);
        assert!(f.includes("src/main.rs"));
        assert!(!f.includes("src/main.test.rs"));
    }

    #[test]
    fn star_does_not_cross_separator() {
        let f = filter(&["*.md"], &[]);
        assert!(f.includes("readme.md"));
        assert!(!f.includes("docs/readme.md"));
    }

    #[test]
    fn bad_glob_is_an_error() {
        let err = Filter::from_patterns(&["[".to_string()], &[]).unwrap_err();
        assert!(format!("{err:#}").contains("invalid glob pattern"));
    }
}
