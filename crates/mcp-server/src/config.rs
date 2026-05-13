//! `TelegramMCP` server configuration: TOML schema, loading, and validation.
//!
//! The server reads a single TOML file at startup and parses it into
//! [`Config`]. Validation rules live in `Config::validate` and run as part
//! of [`Config::load`].

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Top-level configuration document.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Bot credentials and Bot API endpoint.
    pub bot: BotConfig,
    /// On-disk storage settings for the `SQLite` history database.
    pub storage: StorageConfig,
    /// Background long-poll updater settings.
    #[serde(default)]
    pub updater: UpdaterConfig,
    /// Retention policy for stored history.
    #[serde(default)]
    #[allow(dead_code, reason = "consumed by retention enforcement in a later task")]
    pub retention: RetentionConfig,
    /// Map of alias names to numeric chat ids.
    #[serde(default)]
    pub aliases: BTreeMap<String, i64>,
    /// Access control: which chats the server may read from and send to.
    #[serde(default)]
    pub access: AccessConfig,
}

/// `[bot]` section: credentials and Bot API endpoint.
#[derive(Debug, Deserialize)]
pub struct BotConfig {
    /// Inline bot token. Discouraged in production; prefer [`Self::token_env`].
    pub token: Option<String>,
    /// Name of an environment variable that holds the bot token.
    pub token_env: Option<String>,
    /// Override for the Telegram Bot API base URL (used for tests).
    pub api_base_url: Option<String>,
}

/// `[storage]` section: where the `SQLite` database lives.
#[derive(Debug, Deserialize)]
pub struct StorageConfig {
    /// Filesystem path of the `SQLite` database file.
    pub path: PathBuf,
}

/// `[updater]` section: background long-poll behaviour.
#[derive(Debug, Deserialize)]
pub struct UpdaterConfig {
    /// When `false`, the server starts without the background updater.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Long-poll timeout in seconds; must be in `[1, 50]`.
    #[serde(default = "default_poll_timeout")]
    pub poll_timeout_secs: u64,
    /// `allowed_updates` list passed to `getUpdates`.
    #[serde(default = "default_allowed_kinds")]
    pub allowed_update_kinds: Vec<String>,
}

impl Default for UpdaterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_timeout_secs: default_poll_timeout(),
            allowed_update_kinds: default_allowed_kinds(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_poll_timeout() -> u64 {
    30
}
fn default_allowed_kinds() -> Vec<String> {
    vec![
        "message".into(),
        "edited_message".into(),
        "channel_post".into(),
        "edited_channel_post".into(),
    ]
}

/// `[retention]` section: optional history-pruning policy.
#[derive(Debug, Default, Deserialize)]
#[allow(dead_code, reason = "consumed by retention enforcement in a later task")]
pub struct RetentionConfig {
    /// Drop messages older than this many days. `None` disables age-based pruning.
    pub max_age_days: Option<u64>,
    /// Hard cap on stored messages. `None` disables count-based pruning.
    pub max_messages_total: Option<i64>,
}

/// `[access]` section: chat allow-lists for read and send tools.
#[derive(Debug, Default, Deserialize)]
pub struct AccessConfig {
    /// Chats the server may read history from. Empty = unrestricted.
    #[serde(default)]
    pub allowed_chats: Vec<AliasOrId>,
    /// Chats the server may send to. Empty = unrestricted.
    #[serde(default)]
    pub allowed_send_targets: Vec<AliasOrId>,
}

/// A chat reference inside `[access]`: either a numeric id or an alias name.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AliasOrId {
    /// Raw numeric Telegram chat id.
    Id(i64),
    /// Alias name; must be present in `[aliases]`.
    Name(String),
}

impl Config {
    /// Load and validate a configuration file from disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, the TOML is malformed,
    /// or [`Config::validate`] rejects the parsed document.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config: {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw).context("parsing config TOML")?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Resolve the bot token from the configured source.
    ///
    /// Prefers `bot.token_env` (an environment variable name); falls back to
    /// the inline `bot.token` while emitting a `tracing::warn!`.
    ///
    /// # Errors
    ///
    /// Returns an error if `token_env` is set but the env var is absent, or
    /// if neither field is configured.
    pub fn resolved_token(&self) -> Result<String> {
        if let Some(env_key) = &self.bot.token_env {
            return std::env::var(env_key).with_context(|| format!("env var {env_key} not set"));
        }
        if let Some(t) = &self.bot.token {
            tracing::warn!("bot.token set inline in config; prefer bot.token_env");
            return Ok(t.clone());
        }
        bail!("must set either [bot] token_env or [bot] token");
    }

    fn validate(&self) -> Result<()> {
        if self.updater.poll_timeout_secs < 1 || self.updater.poll_timeout_secs > 50 {
            bail!("[updater] poll_timeout_secs must be in [1, 50]");
        }
        if let Some(parent) = self.storage.path.parent() {
            let missing_parent = !parent.as_os_str().is_empty() && !parent.exists();
            if missing_parent {
                bail!("storage.path parent does not exist: {}", parent.display());
            }
        }
        for entry in self
            .access
            .allowed_chats
            .iter()
            .chain(self.access.allowed_send_targets.iter())
        {
            let AliasOrId::Name(n) = entry else { continue };
            if !self.aliases.contains_key(n) {
                bail!("access list references unknown alias: {n}");
            }
        }
        if self.bot.token.is_none() && self.bot.token_env.is_none() {
            bail!("[bot] requires either token or token_env");
        }
        Ok(())
    }

    /// Resolve a list of [`AliasOrId`] entries to numeric chat ids using the
    /// configured `[aliases]` table.
    ///
    /// # Errors
    ///
    /// Returns an error if any [`AliasOrId::Name`] is not present in the
    /// alias table.
    pub fn resolve_id_list(&self, list: &[AliasOrId]) -> Result<Vec<i64>> {
        list.iter()
            .map(|e| match e {
                AliasOrId::Id(id) => Ok(*id),
                AliasOrId::Name(n) => self
                    .aliases
                    .get(n)
                    .copied()
                    .with_context(|| format!("unknown alias in access list: {n}")),
            })
            .collect()
    }
}
