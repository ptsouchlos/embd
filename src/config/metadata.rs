use serde::{Deserialize, Serialize};

/// What `commit_hash` was resolved from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tracking {
    /// The pin came from a branch; the string is the branch name.
    Branch(String),
    /// The pin came from a tag; the string is the tag name.
    Tag(String),
}

/// Represents the metadata stored for a single entry in `embd`'s configuration file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub remote: String,
    pub commit_hash: String,
    /// What `commit_hash` was resolved from. `None` means a raw commit hash, or
    /// an entry written before tracking existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking: Option<Tracking>,
    pub allow_untracked: bool,
    /// Glob patterns; when non-empty, only matching files are pulled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Glob patterns; matching files are never pulled (excludes win over includes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(tracking: Option<Tracking>) -> Metadata {
        Metadata {
            remote: "https://example.git".into(),
            commit_hash: "abc123".into(),
            tracking,
            allow_untracked: false,
            include: Vec::new(),
            exclude: Vec::new(),
        }
    }

    #[test]
    fn loads_without_tracking_key() {
        // Entries written before tracking existed have no key at all.
        let m: Metadata = toml::from_str(
            r#"
remote = "https://example.git"
commit_hash = "abc123"
allow_untracked = false
"#,
        )
        .unwrap();
        assert_eq!(m.tracking, None);
    }

    #[test]
    fn saves_without_tracking_key_when_none() {
        let s = toml::to_string_pretty(&metadata(None)).unwrap();
        assert!(!s.contains("tracking"), "got:\n{s}");
    }

    #[test]
    fn branch_tracking_round_trips() {
        let s = toml::to_string_pretty(&metadata(Some(Tracking::Branch("main".into())))).unwrap();
        // toml's pretty serializer renders a nested enum as a table section,
        // not an inline table.
        assert!(s.contains("[tracking]"), "got:\n{s}");
        assert!(s.contains("branch = \"main\""), "got:\n{s}");
        let m: Metadata = toml::from_str(&s).unwrap();
        assert_eq!(m.tracking, Some(Tracking::Branch("main".into())));
    }

    #[test]
    fn tag_tracking_round_trips() {
        let s = toml::to_string_pretty(&metadata(Some(Tracking::Tag("v1.0".into())))).unwrap();
        assert!(s.contains("[tracking]"), "got:\n{s}");
        assert!(s.contains("tag = \"v1.0\""), "got:\n{s}");
        let m: Metadata = toml::from_str(&s).unwrap();
        assert_eq!(m.tracking, Some(Tracking::Tag("v1.0".into())));
    }
}
