//! `TelegramMCP` — MCP server binary, stdio transport.
//!
//! Wires the `rmcp` request loop on top of the modules introduced in Task 17.
//! Startup parses `--config <path>`, loads and validates the TOML config,
//! constructs the [`TgClient`] / [`History`] / [`Aliases`] runtime state,
//! optionally spawns the background updater, then drives MCP requests over
//! stdio. Tool implementations are added incrementally; this task wires
//! `tg_bot_whoami` and `tg_bot_list_aliases`.

mod config;
mod error;
mod tools_io;

use anyhow::{Context, Result};
use rmcp::{
    Error as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParam, CallToolResult, Content, Implementation, ListToolsResult,
        PaginatedRequestParam, ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    transport::stdio,
};
use schemars::JsonSchema;
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::error::{alias_err_to_mcp, client_err_to_mcp};
use crate::tools_io::{BotWhoamiInput, ListAliasesInput};

use aliases::{Aliases, ChatRef};
use history::History;
use tg_client::TgClient;

/// Long-lived state shared by every tool invocation.
#[derive(Clone)]
struct State {
    /// Outbound Telegram Bot API client.
    bot: TgClient,
    /// Local message history store.
    #[allow(dead_code, reason = "consumed by history tools landing in Tasks 19-21")]
    store: History,
    /// Chat-name alias table loaded from `[aliases]`.
    aliases: Aliases,
    /// Resolved allow-list for send tools. `None` means unrestricted.
    #[allow(dead_code, reason = "consumed by send tools landing in Tasks 19-21")]
    allowed_send_targets: Option<Vec<i64>>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("bot", &self.bot)
            .field("allowed_send_targets", &self.allowed_send_targets)
            .finish_non_exhaustive()
    }
}

/// MCP server handle wrapped around the shared [`State`].
#[derive(Debug, Clone)]
struct Server(Arc<State>);

/// Build a JSON Schema object suitable for [`Tool::input_schema`].
fn schema_obj<T: JsonSchema>() -> Arc<Map<String, Value>> {
    let mut generator = schemars::r#gen::SchemaGenerator::default();
    let schema = T::json_schema(&mut generator);
    let value = serde_json::to_value(&schema).expect("schema serializes");
    let mut obj = match value {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    obj.remove("$schema");
    obj.remove("title");
    Arc::new(obj)
}

/// Construct a [`Tool`] registry entry.
fn tool(name: &'static str, desc: &'static str, schema: Arc<Map<String, Value>>) -> Tool {
    Tool {
        name: name.into(),
        description: Some(desc.into()),
        input_schema: schema,
        annotations: None,
    }
}

/// Decode the `arguments` map of a [`CallToolRequestParam`] into a typed input.
fn parse_args<T: serde::de::DeserializeOwned>(
    args: Option<&Map<String, Value>>,
) -> Result<T, McpError> {
    let value = args.map_or(Value::Object(Map::new()), |m| Value::Object(m.clone()));
    serde_json::from_value(value)
        .map_err(|e| McpError::invalid_params(format!("invalid arguments: {e}"), None))
}

/// Serialise `v` as pretty JSON and wrap it as a successful [`CallToolResult`].
fn ok_json<T: serde::Serialize>(v: &T) -> Result<CallToolResult, McpError> {
    let payload = serde_json::to_string_pretty(v)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(payload)]))
}

/// Resolve a [`ChatRef`] against the configured alias table.
#[allow(dead_code, reason = "consumed by chat tools landing in Tasks 19-21")]
fn resolve_chat(aliases: &Aliases, r: &ChatRef) -> Result<i64, McpError> {
    aliases.resolve(r).map_err(|e| alias_err_to_mcp(&e))
}

/// Enforce the `[access] allowed_send_targets` allow-list for outbound tools.
#[allow(dead_code, reason = "consumed by send tools landing in Tasks 19-21")]
fn check_send_allowed(state: &State, chat_id: i64) -> Result<(), McpError> {
    if let Some(list) = &state.allowed_send_targets {
        if !list.contains(&chat_id) {
            return Err(McpError::invalid_params(
                format!("chat {chat_id} is not in allowed_send_targets"),
                None,
            ));
        }
    }
    Ok(())
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "TelegramMCP".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            instructions: Some(
                "Telegram Bot API + local history. Send messages, read incoming, \
                 search history."
                    .into(),
            ),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: vec![
                tool(
                    "tg_bot_whoami",
                    "Return bot id, username, and display name.",
                    schema_obj::<BotWhoamiInput>(),
                ),
                tool(
                    "tg_bot_list_aliases",
                    "Return configured chat-name -> chat_id map.",
                    schema_obj::<ListAliasesInput>(),
                ),
            ],
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        match request.name.as_ref() {
            "tg_bot_whoami" => {
                let _: BotWhoamiInput = parse_args(request.arguments.as_ref())?;
                let me = self
                    .0
                    .bot
                    .get_me()
                    .await
                    .map_err(|e| client_err_to_mcp(&e))?;
                ok_json(&me)
            }
            "tg_bot_list_aliases" => {
                let _: ListAliasesInput = parse_args(request.arguments.as_ref())?;
                ok_json(self.0.aliases.as_map())
            }
            other => Err(McpError::invalid_params(
                format!("unknown tool: {other}"),
                None,
            )),
        }
    }
}

/// Minimal `--config <path>` parser. Anything else is rejected so we don't
/// silently swallow flags users intended for the binary.
fn parse_cli() -> Result<Option<PathBuf>> {
    let mut args = std::env::args().skip(1);
    let mut cfg = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => {
                let v = args.next().context("--config requires a path argument")?;
                cfg = Some(PathBuf::from(v));
            }
            "--help" | "-h" => {
                eprintln!(
                    "TelegramMCP v{} - MCP server for the Telegram Bot API.\n\
                     \n\
                     USAGE:\n  TelegramMCP --config <path>\n\
                     \n\
                     ENV:\n  TELEGRAM_MCP_LOG    tracing-subscriber filter (default: info).",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    Ok(cfg)
}

#[tokio::main]
#[allow(
    clippy::too_many_lines,
    reason = "linear startup wiring; splitting per-section obscures the flow"
)]
async fn main() -> Result<()> {
    let filter =
        EnvFilter::try_from_env("TELEGRAM_MCP_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let config_path = parse_cli()?.context("--config <path> is required")?;
    let cfg = Config::load(&config_path)?;

    let token = cfg.resolved_token()?;
    let api_base = cfg
        .bot
        .api_base_url
        .as_deref()
        .map(url::Url::parse)
        .transpose()
        .context("invalid bot.api_base_url")?;
    let client = TgClient::new(token, api_base).context("constructing TgClient")?;

    let store = History::open(&cfg.storage.path)
        .with_context(|| format!("opening history at {}", cfg.storage.path.display()))?;

    let aliases = Aliases::new(cfg.aliases.clone());

    let allowed_send_targets = if cfg.access.allowed_send_targets.is_empty() {
        None
    } else {
        Some(cfg.resolve_id_list(&cfg.access.allowed_send_targets)?)
    };

    let state = Arc::new(State {
        bot: client.clone(),
        store: store.clone(),
        aliases,
        allowed_send_targets,
    });

    if cfg.updater.enabled {
        let allowed_chats = if cfg.access.allowed_chats.is_empty() {
            None
        } else {
            Some(cfg.resolve_id_list(&cfg.access.allowed_chats)?)
        };
        let updater_cfg = tg_updater::UpdaterConfig {
            poll_timeout_secs: cfg.updater.poll_timeout_secs,
            allowed_update_kinds: cfg.updater.allowed_update_kinds.clone(),
            allowed_chats,
        };
        let updater = tg_updater::Updater {
            client,
            store,
            config: updater_cfg,
        };
        tokio::spawn(async move {
            match updater.run().await {
                Ok(never) => match never {},
                Err(e) => tracing::error!(error = %e, "updater loop terminated"),
            }
        });
    }

    let server = Server(state);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
