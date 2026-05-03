//! Persistent configuration for `horologium status`.
//!
//! File: `~/.config/horologium/config.toml`
//! CLI flags (`--powerline`, etc.) override the corresponding config values.

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Top-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub render: RenderConfig,
    pub segments: Vec<SegmentConfig>,
    pub thresholds: ThresholdConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RenderConfig {
    pub powerline: bool,
    pub multiline: bool,
    pub hyperlinks: bool,
}

/// A segment in the statusline. Order in the array = display order.
/// Remove an entry to hide it; reorder to rearrange.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentConfig {
    pub name: SegmentName,
    /// Powerline background color (xterm-256 index). `None` = use default.
    pub bg: Option<u8>,
    /// Powerline foreground color (xterm-256 index). `None` = use default.
    pub fg: Option<u8>,
    /// Row assignment for multiline mode. 0 = identity, 1 = usage.
    /// `None` = use default.
    pub row: Option<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SegmentName {
    Model,
    Dir,
    Branch,
    Context,
    Cost,
    #[serde(rename = "rate_5h")]
    Rate5h,
    #[serde(rename = "rate_7d")]
    Rate7d,
}

impl SegmentName {
    pub const ALL_DEFAULT_ORDER: &[SegmentName] = &[
        SegmentName::Model,
        SegmentName::Dir,
        SegmentName::Branch,
        SegmentName::Context,
        SegmentName::Cost,
        SegmentName::Rate5h,
        SegmentName::Rate7d,
    ];
}

/// Color thresholds for rate-limit segments. Below `green_below` is green,
/// `green_below..red_above` is yellow, `red_above..=100` is red.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThresholdConfig {
    pub green_below: i64,
    pub red_above: i64,
}

// ── Defaults ────────────────────────────────────────────────────────

impl Default for Config {
    fn default() -> Self {
        Self {
            render: RenderConfig::default(),
            segments: default_segments(),
            thresholds: ThresholdConfig::default(),
        }
    }
}

impl Default for ThresholdConfig {
    fn default() -> Self {
        Self {
            green_below: 70,
            red_above: 90,
        }
    }
}

fn default_segments() -> Vec<SegmentConfig> {
    SegmentName::ALL_DEFAULT_ORDER
        .iter()
        .map(|&name| SegmentConfig {
            name,
            bg: None,
            fg: None,
            row: None,
        })
        .collect()
}

// ── Segment defaults (matching status.rs hardcoded values) ──────────

pub struct SegmentDefaults {
    pub bg: u8,
    pub fg: u8,
    pub row: u8,
}

impl SegmentName {
    pub fn defaults(self) -> SegmentDefaults {
        match self {
            SegmentName::Model => SegmentDefaults {
                bg: 24,
                fg: 15,
                row: 0,
            },
            SegmentName::Dir => SegmentDefaults {
                bg: 31,
                fg: 15,
                row: 0,
            },
            SegmentName::Branch => SegmentDefaults {
                bg: 22,
                fg: 15,
                row: 0,
            },
            SegmentName::Context => SegmentDefaults {
                bg: 237,
                fg: 15,
                row: 1,
            },
            SegmentName::Cost => SegmentDefaults {
                bg: 90,
                fg: 15,
                row: 1,
            },
            SegmentName::Rate5h | SegmentName::Rate7d => SegmentDefaults {
                bg: 22,
                fg: 15,
                row: 1,
            },
        }
    }
}

impl SegmentConfig {
    pub fn resolved_bg(&self) -> u8 {
        self.bg.unwrap_or(self.name.defaults().bg)
    }

    pub fn resolved_fg(&self) -> u8 {
        self.fg.unwrap_or(self.name.defaults().fg)
    }

    pub fn resolved_row(&self) -> u8 {
        self.row.unwrap_or(self.name.defaults().row)
    }
}

// ── File I/O ────────────────────────────────────────────────────────

pub fn config_path() -> Option<PathBuf> {
    config_path_from_env(
        std::env::var_os("HOROLOGIUM_CONFIG"),
        std::env::var_os("HOME"),
    )
}

fn config_path_from_env(
    config_override: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    if let Some(path) = config_override {
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }

    let home = home?;
    Some(PathBuf::from(home).join(".config/horologium/config.toml"))
}

pub fn load(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read config `{}`", path.display()))?;
    let cfg: Config =
        toml::from_str(&text).with_context(|| format!("parse config `{}`", path.display()))?;
    Ok(cfg)
}

pub fn load_default_path() -> Result<Config> {
    match config_path() {
        Some(p) => load(&p),
        None => Ok(Config::default()),
    }
}

/// Validate a config and return a list of warnings (non-fatal) and errors.
pub fn validate(cfg: &Config) -> Vec<String> {
    let mut issues = Vec::new();

    if cfg.thresholds.green_below >= cfg.thresholds.red_above {
        issues.push(format!(
            "thresholds: green_below ({}) must be < red_above ({})",
            cfg.thresholds.green_below, cfg.thresholds.red_above,
        ));
    }

    if cfg.segments.is_empty() {
        issues.push("segments: list is empty — statusline will be blank".into());
    }

    let mut seen = std::collections::HashSet::new();
    for seg in &cfg.segments {
        if !seen.insert(seg.name) {
            issues.push(format!("segments: duplicate `{:?}`", seg.name));
        }
    }

    issues
}

/// Generate the default config as a commented TOML string.
pub fn generate_default_toml() -> String {
    let header = r#"# Horologium statusline configuration
# Location: ~/.config/horologium/config.toml
# Override path with HOROLOGIUM_CONFIG=/path/to/config.toml when needed.
# CLI flags (--powerline, --multiline, --hyperlinks) override these values.

"#;
    let cfg = Config::default();
    let body = toml::to_string_pretty(&cfg).unwrap_or_default();
    format!("{}{}", header, body)
}

// ── CLI ─────────────────────────────────────────────────────────────

#[derive(Args)]
pub struct ConfigureArgs {
    #[command(subcommand)]
    command: ConfigureCommand,
}

#[derive(Subcommand)]
enum ConfigureCommand {
    /// Generate a default config file with comments.
    Init {
        /// Overwrite existing config without prompting.
        #[arg(long)]
        force: bool,
    },
    /// Validate the current config file.
    Check,
    /// Print the resolved config path.
    Path,
}

pub fn run(args: ConfigureArgs) -> Result<()> {
    match args.command {
        ConfigureCommand::Init { force } => cmd_init(force),
        ConfigureCommand::Check => cmd_check(),
        ConfigureCommand::Path => cmd_path(),
    }
}

fn cmd_init(force: bool) -> Result<()> {
    let path = config_path().ok_or_else(|| anyhow!("$HOME not set"))?;

    if path.exists() && !force {
        eprintln!("config already exists: {}", path.display());
        eprintln!("use --force to overwrite");
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir `{}`", parent.display()))?;
    }

    let content = generate_default_toml();
    std::fs::write(&path, &content).with_context(|| format!("write `{}`", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

fn cmd_check() -> Result<()> {
    let path = config_path().ok_or_else(|| anyhow!("$HOME not set"))?;

    if !path.exists() {
        println!("no config file found at {}", path.display());
        println!("run `horologium configure init` to create one");
        return Ok(());
    }

    let cfg = load(&path)?;
    let issues = validate(&cfg);

    if issues.is_empty() {
        println!("config OK: {}", path.display());
    } else {
        println!("config issues in {}:", path.display());
        for issue in &issues {
            println!("  - {}", issue);
        }
        return Err(anyhow!("config has {} issue(s)", issues.len()));
    }
    Ok(())
}

fn cmd_path() -> Result<()> {
    let path = config_path().ok_or_else(|| anyhow!("$HOME not set"))?;
    println!("{}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_roundtrips_through_toml() {
        let cfg = Config::default();
        let s = toml::to_string_pretty(&cfg).unwrap();
        let parsed: Config = toml::from_str(&s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn empty_toml_gives_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let cfg: Config = toml::from_str(
            r#"
            [render]
            powerline = true
            "#,
        )
        .unwrap();
        assert!(cfg.render.powerline);
        assert!(!cfg.render.multiline);
        assert_eq!(cfg.segments.len(), 7);
    }

    #[test]
    fn custom_segment_order() {
        let cfg: Config = toml::from_str(
            r#"
            [[segments]]
            name = "cost"
            bg = 200

            [[segments]]
            name = "model"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.segments.len(), 2);
        assert_eq!(cfg.segments[0].name, SegmentName::Cost);
        assert_eq!(cfg.segments[0].bg, Some(200));
        assert_eq!(cfg.segments[1].name, SegmentName::Model);
        assert_eq!(cfg.segments[1].bg, None);
    }

    #[test]
    fn resolved_colors_use_defaults_when_none() {
        let seg = SegmentConfig {
            name: SegmentName::Model,
            bg: None,
            fg: None,
            row: None,
        };
        assert_eq!(seg.resolved_bg(), 24);
        assert_eq!(seg.resolved_fg(), 15);
        assert_eq!(seg.resolved_row(), 0);
    }

    #[test]
    fn resolved_colors_use_override_when_set() {
        let seg = SegmentConfig {
            name: SegmentName::Model,
            bg: Some(100),
            fg: Some(0),
            row: Some(1),
        };
        assert_eq!(seg.resolved_bg(), 100);
        assert_eq!(seg.resolved_fg(), 0);
        assert_eq!(seg.resolved_row(), 1);
    }

    #[test]
    fn validate_catches_bad_thresholds() {
        let mut cfg = Config::default();
        cfg.thresholds.green_below = 90;
        cfg.thresholds.red_above = 70;
        let issues = validate(&cfg);
        assert!(issues.iter().any(|i| i.contains("green_below")));
    }

    #[test]
    fn validate_catches_empty_segments() {
        let mut cfg = Config::default();
        cfg.segments.clear();
        let issues = validate(&cfg);
        assert!(issues.iter().any(|i| i.contains("empty")));
    }

    #[test]
    fn validate_catches_duplicates() {
        let mut cfg = Config::default();
        cfg.segments.push(SegmentConfig {
            name: SegmentName::Model,
            bg: None,
            fg: None,
            row: None,
        });
        let issues = validate(&cfg);
        assert!(issues.iter().any(|i| i.contains("duplicate")));
    }

    #[test]
    fn validate_passes_default_config() {
        let issues = validate(&Config::default());
        assert!(
            issues.is_empty(),
            "default config should be valid: {:?}",
            issues
        );
    }

    #[test]
    fn config_path_uses_override_when_set() {
        let path = config_path_from_env(
            Some(OsString::from("/tmp/horo.toml")),
            Some(OsString::from("/home/alice")),
        );
        assert_eq!(path, Some(PathBuf::from("/tmp/horo.toml")));
    }

    #[test]
    fn config_path_ignores_empty_override() {
        let path = config_path_from_env(Some(OsString::new()), Some(OsString::from("/home/alice")));
        assert_eq!(
            path,
            Some(PathBuf::from("/home/alice/.config/horologium/config.toml"))
        );
    }

    #[test]
    fn load_missing_file_returns_default() {
        let cfg = load(Path::new("/nonexistent/config.toml")).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn generate_default_toml_is_parseable() {
        let s = generate_default_toml();
        let _cfg: Config = toml::from_str(&s).unwrap();
    }
}
