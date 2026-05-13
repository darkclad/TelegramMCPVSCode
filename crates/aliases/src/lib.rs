//! Chat-name alias resolution.

use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::BTreeMap;
use thiserror::Error;

/// Caller-supplied reference to a Telegram chat: either the raw numeric id
/// or a name that resolves through configured aliases.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ChatRef {
    /// Raw numeric Telegram chat id.
    Id(i64),
    /// Alias name to be resolved through an [`Aliases`] table.
    Name(String),
}

/// Configured alias table. Construct with [`Aliases::new`]; the server
/// loads one from the `[aliases]` config block at startup.
#[derive(Debug, Clone, Default)]
pub struct Aliases {
    map: BTreeMap<String, i64>,
}

impl Aliases {
    /// Build an alias table from a map of names to numeric chat ids.
    #[must_use]
    pub fn new(map: BTreeMap<String, i64>) -> Self {
        Self { map }
    }

    /// Resolve a [`ChatRef`] to a numeric chat id, looking up names in the
    /// configured alias table.
    ///
    /// # Errors
    ///
    /// Returns [`UnknownAlias`] if the reference is a name that is not
    /// present in the table.
    pub fn resolve(&self, r: &ChatRef) -> Result<i64, UnknownAlias> {
        match r {
            ChatRef::Id(id) => Ok(*id),
            ChatRef::Name(n) => self
                .map
                .get(n)
                .copied()
                .ok_or_else(|| UnknownAlias { name: n.clone() }),
        }
    }

    /// Iterate over the configured alias names in sorted order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.map.keys().map(String::as_str)
    }

    /// Borrow the underlying name-to-id map.
    #[must_use]
    pub fn as_map(&self) -> &BTreeMap<String, i64> {
        &self.map
    }
}

/// Error returned by [`Aliases::resolve`] when a name is not in the table.
#[derive(Debug, Error)]
#[error("unknown chat alias: {name}")]
pub struct UnknownAlias {
    /// The alias name that could not be resolved.
    pub name: String,
}
