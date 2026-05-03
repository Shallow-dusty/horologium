use clap::ValueEnum;
use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Source {
    Claude,
    Codex,
}

impl Source {
    pub fn default_root(self) -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        let home = PathBuf::from(home);
        match self {
            Source::Claude => Some(home.join(".claude/projects")),
            Source::Codex => Some(home.join(".codex/sessions")),
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Claude => f.write_str("claude"),
            Source::Codex => f.write_str("codex"),
        }
    }
}
