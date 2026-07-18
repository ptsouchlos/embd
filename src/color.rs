use owo_colors::OwoColorize;

/// User-selectable color behavior, exposed as `--color <auto|always|never>`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default)]
pub(crate) enum ColorMode {
    /// Color when stdout/stderr is a terminal and NO_COLOR/CLICOLOR aren't set.
    #[default]
    Auto,
    /// Always emit color, even when piped.
    Always,
    /// Never emit color.
    Never,
}

impl ColorMode {
    /// Push this choice into anstream's global color state. Must run before any
    /// output is produced; `anstream::{print,eprint}ln!` consult it on every call.
    pub(crate) fn apply(self) {
        let choice = match self {
            ColorMode::Auto => anstream::ColorChoice::Auto,
            ColorMode::Always => anstream::ColorChoice::Always,
            ColorMode::Never => anstream::ColorChoice::Never,
        };
        choice.write_global();
    }
}

/// Styled "error:" label.
pub(crate) fn error_label() -> String {
    "error:".red().bold().to_string()
}

/// Styled "warning:" label.
pub(crate) fn warning_label() -> String {
    "warning:".yellow().bold().to_string()
}

/// Green: state matches what's expected, nothing to do.
pub(crate) fn ok(s: &str) -> String {
    s.green().to_string()
}

/// Yellow: local drift that hasn't been applied/resolved yet.
pub(crate) fn warn(s: &str) -> String {
    s.yellow().to_string()
}

/// Red: missing state or a condition that blocks the requested action.
pub(crate) fn bad(s: &str) -> String {
    s.red().to_string()
}

/// Bold: entry headers (`name (folder)`).
pub(crate) fn header(s: &str) -> String {
    s.bold().to_string()
}

/// Dim: secondary detail, e.g. commit hashes.
pub(crate) fn dim(s: &str) -> String {
    s.dimmed().to_string()
}

/// Single-letter file-status markers, colored to match `git status --short`
/// conventions where they overlap (M yellow, D red, ? cyan).
pub(crate) fn marker(letter: char) -> String {
    match letter {
        'M' => letter.yellow().to_string(),
        'D' | 'X' => letter.red().to_string(),
        '?' => letter.cyan().to_string(),
        'L' => letter.magenta().to_string(),
        'W' => letter.green().to_string(),
        _ => letter.to_string(),
    }
}
