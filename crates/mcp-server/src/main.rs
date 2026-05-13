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
use crate::error::{alias_err_to_mcp, client_err_to_mcp, history_err_to_mcp};
use crate::tools_io::{
    BotWhoamiInput, ChatActionInput, DeleteMessageInput, DownloadInput, EditMessageInput,
    ForwardMessageInput, GetChatInput, GetMessageInput, HistoryMessagesInput, HistorySearchInput,
    ListAliasesInput, ListChatsInput, MarkReadInput, SendDocumentInput, SendMessageInput,
    SendMessageOutput, SendPhotoInput,
};

use aliases::{Aliases, ChatRef};
use history::History;
use tg_client::TgClient;

/// Long-lived state shared by every tool invocation.
#[derive(Clone)]
struct State {
    /// Outbound Telegram Bot API client.
    bot: TgClient,
    /// Local message history store.
    store: History,
    /// Chat-name alias table loaded from `[aliases]`.
    aliases: Aliases,
    /// Resolved allow-list for send tools. `None` means unrestricted.
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
fn resolve_chat(aliases: &Aliases, r: &ChatRef) -> Result<i64, McpError> {
    aliases.resolve(r).map_err(|e| alias_err_to_mcp(&e))
}

/// Enforce the `[access] allowed_send_targets` allow-list for outbound tools.
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

/// Map a user-supplied `parse_mode` string to teloxide's [`ParseMode`].
///
/// Recognises `"markdown"` and `"markdownv2"` (both map to `MarkdownV2`) and
/// `"html"` case-insensitively. Anything else — including `None`, the empty
/// string, or unknown variants — returns `None`, meaning no parse mode is
/// applied and Telegram treats the body as plain text.
///
/// [`ParseMode`]: teloxide::types::ParseMode
fn parse_parse_mode(s: &str) -> Option<teloxide::types::ParseMode> {
    use teloxide::types::ParseMode;
    match s.to_ascii_lowercase().as_str() {
        "markdown" | "markdownv2" => Some(ParseMode::MarkdownV2),
        "html" => Some(ParseMode::Html),
        _ => None,
    }
}

/// Map a Bot API chat-action string to teloxide's [`ChatAction`].
///
/// Matching is case-sensitive: the LLM-facing tool input must use the exact
/// Bot API spellings (e.g. `"typing"`, `"upload_photo"`). Unknown values
/// return `None` so the caller can surface a clear "unknown action" error.
///
/// [`ChatAction`]: teloxide::types::ChatAction
fn chat_action_from_str(s: &str) -> Option<teloxide::types::ChatAction> {
    use teloxide::types::ChatAction as A;
    Some(match s {
        "typing" => A::Typing,
        "upload_photo" => A::UploadPhoto,
        "record_video" => A::RecordVideo,
        "upload_video" => A::UploadVideo,
        "record_voice" => A::RecordVoice,
        "upload_voice" => A::UploadVoice,
        "upload_document" => A::UploadDocument,
        "find_location" => A::FindLocation,
        "record_video_note" => A::RecordVideoNote,
        "upload_video_note" => A::UploadVideoNote,
        _ => return None,
    })
}

/// Mirror an outbound Bot API success into local history.
///
/// Used by every send-side tool (`tg_send_message`, `tg_send_photo`, ...) so
/// that messages we originate appear in history with `direction = 'out'`.
/// The helper bumps `last_seen` on an existing chat row via
/// [`History::touch_chat_last_seen`] (it does not create one — we have no
/// reliable [`history::ChatKind`] for send-only chats) and then writes a
/// single [`StoredMessage`] row. `text` is the optional plain-text body or
/// caption, `media_kind` is the media tag (e.g. `"photo"`, `"document"`),
/// and `reply_to` propagates `reply_to_message_id` when the caller knows
/// it — the Bot API's send response does not always echo it back.
///
/// The chat row is created the first time an inbound update arrives via
/// the updater, at which point the real [`history::ChatKind`] is known.
/// Outbound-only chats therefore do not appear in `tg_history_list_chats`
/// until an inbound message arrives — matching the spec's intent that
/// `list_chats` reports chats the bot has interacted with.
///
/// [`StoredMessage`]: history::StoredMessage
async fn mirror_outbound(
    store: &History,
    sent: &tg_client::SentMessage,
    text: Option<&str>,
    media_kind: Option<&str>,
    reply_to: Option<i64>,
) -> Result<(), McpError> {
    store
        .touch_chat_last_seen(sent.chat_id, sent.date)
        .await
        .map_err(|e| history_err_to_mcp(&e))?;
    store
        .insert_message(&history::StoredMessage {
            chat_id: sent.chat_id,
            message_id: sent.message_id,
            date: sent.date,
            from_id: None,
            from_name: None,
            reply_to,
            text: text.map(str::to_string),
            media_kind: media_kind.map(str::to_string),
            media_file_id: None,
            media_meta: None,
            direction: history::Direction::Out,
            raw: serde_json::json!({ "outbound": true }),
        })
        .await
        .map_err(|e| history_err_to_mcp(&e))?;
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

    #[allow(
        clippy::too_many_lines,
        reason = "tool registry; per-tool entries read better as one flat list"
    )]
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
                tool(
                    "tg_send_message",
                    "Send a text message to a chat. `chat` accepts a numeric chat_id or a \
                     configured alias. Returns the sent message id + date. The message is also \
                     written to local history with direction='out'.",
                    schema_obj::<SendMessageInput>(),
                ),
                tool(
                    "tg_send_photo",
                    "Send a photo from a local path. Caption optional. Mirrors to history.",
                    schema_obj::<SendPhotoInput>(),
                ),
                tool(
                    "tg_send_document",
                    "Send a document (any file) from a local path. Caption + custom filename \
                     optional.",
                    schema_obj::<SendDocumentInput>(),
                ),
                tool(
                    "tg_edit_message",
                    "Edit the text of a previously-sent message. Returns the updated message \
                     stamp.",
                    schema_obj::<EditMessageInput>(),
                ),
                tool(
                    "tg_delete_message",
                    "Delete a message by chat + message_id. Bot can only delete its own messages \
                     (or other users' messages if it has admin rights).",
                    schema_obj::<DeleteMessageInput>(),
                ),
                tool(
                    "tg_forward_message",
                    "Forward a message from one chat to another. Returns the forwarded message \
                     id.",
                    schema_obj::<ForwardMessageInput>(),
                ),
                tool(
                    "tg_send_chat_action",
                    "Show a 'typing'/'uploading'/etc. indicator in the chat for ~5s. \
                     action: typing | upload_photo | record_video | upload_video | record_voice \
                     | upload_voice | upload_document | find_location | record_video_note | \
                     upload_video_note",
                    schema_obj::<ChatActionInput>(),
                ),
                tool(
                    "tg_history_list_chats",
                    "List chats the bot has seen, with last-message timestamp + unread count.",
                    schema_obj::<ListChatsInput>(),
                ),
                tool(
                    "tg_history_get_chat",
                    "Return metadata for one chat: kind, title, username, first/last seen.",
                    schema_obj::<GetChatInput>(),
                ),
                tool(
                    "tg_history_messages",
                    "Paginated messages from a chat, newest-first. before_message_id and \
                     after_message_id are message-id cursors, exclusive. limit defaults to 50, \
                     clamped to [1, 500].",
                    schema_obj::<HistoryMessagesInput>(),
                ),
                tool(
                    "tg_history_search",
                    "Full-text search across stored messages (FTS5). Optionally scope to a chat \
                     or time window (unix seconds).",
                    schema_obj::<HistorySearchInput>(),
                ),
                tool(
                    "tg_history_get_message",
                    "Fetch a single stored message by (chat, message_id).",
                    schema_obj::<GetMessageInput>(),
                ),
                tool(
                    "tg_history_mark_read",
                    "Move the local unread baseline to this message_id, so subsequent \
                     list_chats reports unread_count from there forward.",
                    schema_obj::<MarkReadInput>(),
                ),
                tool(
                    "tg_history_download",
                    "Download the media attached to a stored message to a local path. Uses the \
                     stored Telegram file_id; fetches bytes from the Bot API on demand.",
                    schema_obj::<DownloadInput>(),
                ),
            ],
            next_cursor: None,
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "single match dispatcher; per-arm splitting would obscure the tool surface"
    )]
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
            "tg_send_message" => {
                let input: SendMessageInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                check_send_allowed(&self.0, chat_id)?;
                let parse_mode = input.parse_mode.as_deref().and_then(parse_parse_mode);
                let sent = self
                    .0
                    .bot
                    .send_message(
                        chat_id,
                        &input.text,
                        parse_mode,
                        input.reply_to,
                        input.silent.unwrap_or(false),
                        input.link_preview.unwrap_or(true),
                    )
                    .await
                    .map_err(|e| client_err_to_mcp(&e))?;
                mirror_outbound(
                    &self.0.store,
                    &sent,
                    Some(&input.text),
                    None,
                    input.reply_to,
                )
                .await?;
                ok_json(&SendMessageOutput {
                    chat_id: sent.chat_id,
                    message_id: sent.message_id,
                    date: sent.date,
                })
            }
            "tg_send_photo" => {
                let input: SendPhotoInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                check_send_allowed(&self.0, chat_id)?;
                let pm = input.parse_mode.as_deref().and_then(parse_parse_mode);
                let sent = self
                    .0
                    .bot
                    .send_photo_path(chat_id, &input.path, input.caption.as_deref(), pm)
                    .await
                    .map_err(|e| client_err_to_mcp(&e))?;
                mirror_outbound(
                    &self.0.store,
                    &sent,
                    input.caption.as_deref(),
                    Some("photo"),
                    None,
                )
                .await?;
                ok_json(&SendMessageOutput {
                    chat_id: sent.chat_id,
                    message_id: sent.message_id,
                    date: sent.date,
                })
            }
            "tg_send_document" => {
                let input: SendDocumentInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                check_send_allowed(&self.0, chat_id)?;
                let sent = self
                    .0
                    .bot
                    .send_document_path(
                        chat_id,
                        &input.path,
                        input.caption.as_deref(),
                        input.filename.as_deref(),
                    )
                    .await
                    .map_err(|e| client_err_to_mcp(&e))?;
                mirror_outbound(
                    &self.0.store,
                    &sent,
                    input.caption.as_deref(),
                    Some("document"),
                    None,
                )
                .await?;
                ok_json(&SendMessageOutput {
                    chat_id: sent.chat_id,
                    message_id: sent.message_id,
                    date: sent.date,
                })
            }
            "tg_edit_message" => {
                let input: EditMessageInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                check_send_allowed(&self.0, chat_id)?;
                let pm = input.parse_mode.as_deref().and_then(parse_parse_mode);
                let sent = self
                    .0
                    .bot
                    .edit_message_text(chat_id, input.message_id, &input.text, pm)
                    .await
                    .map_err(|e| client_err_to_mcp(&e))?;
                mirror_outbound(&self.0.store, &sent, Some(&input.text), None, None).await?;
                ok_json(&SendMessageOutput {
                    chat_id: sent.chat_id,
                    message_id: sent.message_id,
                    date: sent.date,
                })
            }
            "tg_delete_message" => {
                let input: DeleteMessageInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                check_send_allowed(&self.0, chat_id)?;
                self.0
                    .bot
                    .delete_message(chat_id, input.message_id)
                    .await
                    .map_err(|e| client_err_to_mcp(&e))?;
                ok_json(&serde_json::json!({
                    "deleted": true,
                    "chat_id": chat_id,
                    "message_id": input.message_id,
                }))
            }
            "tg_forward_message" => {
                let input: ForwardMessageInput = parse_args(request.arguments.as_ref())?;
                let from_chat = resolve_chat(&self.0.aliases, &input.from_chat)?;
                let to_chat = resolve_chat(&self.0.aliases, &input.to_chat)?;
                check_send_allowed(&self.0, to_chat)?;
                let sent = self
                    .0
                    .bot
                    .forward_message(from_chat, input.message_id, to_chat)
                    .await
                    .map_err(|e| client_err_to_mcp(&e))?;
                mirror_outbound(&self.0.store, &sent, None, None, None).await?;
                ok_json(&SendMessageOutput {
                    chat_id: sent.chat_id,
                    message_id: sent.message_id,
                    date: sent.date,
                })
            }
            "tg_send_chat_action" => {
                let input: ChatActionInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                check_send_allowed(&self.0, chat_id)?;
                let action = chat_action_from_str(&input.action).ok_or_else(|| {
                    McpError::invalid_params(format!("unknown action: {}", input.action), None)
                })?;
                self.0
                    .bot
                    .send_chat_action(chat_id, action)
                    .await
                    .map_err(|e| client_err_to_mcp(&e))?;
                ok_json(&serde_json::json!({ "ok": true }))
            }
            "tg_history_list_chats" => {
                let _: ListChatsInput = parse_args(request.arguments.as_ref())?;
                let chats = self
                    .0
                    .store
                    .list_chats()
                    .await
                    .map_err(|e| history_err_to_mcp(&e))?;
                ok_json(&chats)
            }
            "tg_history_get_chat" => {
                let input: GetChatInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                let c = self
                    .0
                    .store
                    .get_chat(chat_id)
                    .await
                    .map_err(|e| history_err_to_mcp(&e))?;
                // `Option<ChatInfo>` serialises as `null` when None — the
                // LLM gets a real "no such chat" answer rather than an
                // error it has to special-case.
                ok_json(&c)
            }
            "tg_history_messages" => {
                let input: HistoryMessagesInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                // Clamp before hitting SQLite: `LIMIT -1` is "no limit" in
                // SQLite, so an unbounded or negative input from a confused
                // (or malicious) LLM could pull millions of rows.
                let limit = input.limit.clamp(1, 500);
                let msgs = self
                    .0
                    .store
                    .messages(
                        chat_id,
                        input.before_message_id,
                        input.after_message_id,
                        limit,
                    )
                    .await
                    .map_err(|e| history_err_to_mcp(&e))?;
                ok_json(&msgs)
            }
            "tg_history_search" => {
                let input: HistorySearchInput = parse_args(request.arguments.as_ref())?;
                let chat_id = match &input.chat {
                    Some(r) => Some(resolve_chat(&self.0.aliases, r)?),
                    None => None,
                };
                let hits = self
                    .0
                    .store
                    .search(&input.query, chat_id, input.since, input.until)
                    .await
                    .map_err(|e| history_err_to_mcp(&e))?;
                ok_json(&hits)
            }
            "tg_history_get_message" => {
                let input: GetMessageInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                let m = self
                    .0
                    .store
                    .get_message(chat_id, input.message_id)
                    .await
                    .map_err(|e| history_err_to_mcp(&e))?;
                ok_json(&m)
            }
            "tg_history_mark_read" => {
                let input: MarkReadInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                self.0
                    .store
                    .mark_read(chat_id, input.up_to_message_id)
                    .await
                    .map_err(|e| history_err_to_mcp(&e))?;
                ok_json(&serde_json::json!({
                    "chat_id": chat_id,
                    "baseline": input.up_to_message_id,
                }))
            }
            "tg_history_download" => {
                let input: DownloadInput = parse_args(request.arguments.as_ref())?;
                let chat_id = resolve_chat(&self.0.aliases, &input.chat)?;
                let m = self
                    .0
                    .store
                    .get_message(chat_id, input.message_id)
                    .await
                    .map_err(|e| history_err_to_mcp(&e))?;
                let file_id = m.media_file_id.as_deref().ok_or_else(|| {
                    McpError::invalid_params(
                        format!("message {chat_id}/{} has no media", input.message_id),
                        None,
                    )
                })?;
                let bytes = self
                    .0
                    .bot
                    .download_file(file_id, &input.dest_path)
                    .await
                    .map_err(|e| client_err_to_mcp(&e))?;
                ok_json(&serde_json::json!({
                    "dest_path": input.dest_path,
                    "bytes": bytes,
                }))
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
