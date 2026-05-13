//! Chat-name alias resolution.
//!
//! Loaded from the `[aliases]` block of `config.toml`. The server resolves
//! every `chat` tool argument through [`Aliases::resolve`] before issuing
//! a Bot API call.

#![allow(missing_docs)] // filled in by later tasks
