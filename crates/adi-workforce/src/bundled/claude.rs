//! Bundled port of `adi.workforce.runner.claude` — the `ClaudeCodeApi`
//! runner: direct Anthropic Messages API calls authenticated like Claude
//! Code itself (config `apiKey` → `ANTHROPIC_API_KEY` →
//! `ANTHROPIC_OAUTH_TOKEN` → Claude Code OAuth tokens from the macOS
//! keychain, refreshed when expired).
//!
//! Dropped from the old plugin: the `PromptRunner` surface (the LLM
//! safety-checker infrastructure was part of the dlopen plugin system) and
//! the tsp-gen macro glue — settings structs are hand-rolled here.

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::config_value::ConfigValue;
use crate::llm::{
    AssistantBlock, AssistantTurn, LlmBackend, LlmRequest, LlmResponse, Turn, UserBlock,
};
use crate::loop_runner_plugin::LoopRunnerPlugin;
use crate::plugin::PluginError;
use crate::tool_def::ToolDef;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// Always-on observability default (under the workforce module dir). Set
/// the `dumpDir` setting or `ADI_CLAUDE_DUMP_DIR` env var to override;
/// "off" disables.
const DUMP_DIR_ENV: &str = "ADI_CLAUDE_DUMP_DIR";
const DEFAULT_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const DEFAULT_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const DEFAULT_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const DEFAULT_OAUTH_SCOPES: &str =
    "user:inference user:profile user:sessions:claude_code user:mcp_servers user:file_upload";
const DEFAULT_OAUTH_BETA_HEADER: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,prompt-caching-scope-2026-01-05,advanced-tool-use-2025-11-20,effort-2025-11-24";
const SDK_VERSION: &str = "0.80.0";
const CLI_VERSION: &str = "2.1.92";

fn default_dump_dir() -> String {
    std::env::var("HOME").map_or_else(
        |_| "/tmp/adi-workforce-dumps".to_string(),
        |h| format!("{h}/.adi/mono/workforce/dumps"),
    )
}

fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    #[allow(clippy::cast_possible_truncation)]
    let r1 = t.as_nanos() as u64;
    let r2 = u64::from(t.subsec_nanos()) ^ 0xdead_beef;
    #[allow(clippy::cast_possible_truncation)]
    {
        format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            (r1 & 0xFFFF_FFFF) as u32,
            ((r1 >> 32) & 0xFFFF) as u16,
            (r2 & 0xFFF) as u16,
            (0x8000 | (r2 >> 12) & 0x3FFF) as u16,
            (r1.wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407))
                & 0xFFFF_FFFF_FFFF,
        )
    }
}

// ── Settings ──

#[derive(Debug, Clone, Default)]
pub struct ClaudeCodeApiSettings {
    pub model: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub keychain_service: Option<String>,
    pub client_id: Option<String>,
    pub token_url: Option<String>,
    pub oauth_scopes: Option<String>,
    pub oauth_beta_header: Option<String>,
    pub effort: Option<String>,
    pub thinking_mode: Option<String>,
    pub thinking_budget: Option<i64>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<i64>,
    pub strict_sequential: Option<bool>,
    pub dump_dir: Option<String>,
}

impl ClaudeCodeApiSettings {
    fn from_config(cfg: &ConfigValue) -> Self {
        let s = |key: &str| {
            cfg.get(key)
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
        };
        #[allow(clippy::cast_possible_truncation)]
        let i = |key: &str| cfg.get(key).and_then(ConfigValue::as_f64).map(|n| n as i64);
        Self {
            // Missing model is caught by `resolve()`, not a panic here.
            model: s("model").unwrap_or_default(),
            api_key: s("apiKey"),
            base_url: s("baseUrl"),
            keychain_service: s("keychainService"),
            client_id: s("clientId"),
            token_url: s("tokenUrl"),
            oauth_scopes: s("oauthScopes"),
            oauth_beta_header: s("oauthBetaHeader"),
            effort: s("effort"),
            thinking_mode: s("thinkingMode"),
            thinking_budget: i("thinkingBudget"),
            temperature: cfg.get("temperature").and_then(ConfigValue::as_f64),
            max_tokens: i("maxTokens"),
            strict_sequential: cfg.get("strictSequential").and_then(ConfigValue::as_bool),
            dump_dir: s("dumpDir"),
        }
    }
}

pub struct ClaudeCodeApi {
    pub settings: ClaudeCodeApiSettings,
}

impl ClaudeCodeApi {
    /// Factory registered under `adi.workforce.runner.claude` /
    /// `ClaudeCodeApi`.
    ///
    /// # Errors
    /// Never fails; the signature matches [`crate::core::RunnerCreateFn`].
    pub fn create(
        config: ConfigValue,
    ) -> Result<std::sync::Arc<dyn LoopRunnerPlugin>, PluginError> {
        Ok(std::sync::Arc::new(Self {
            settings: ClaudeCodeApiSettings::from_config(&config),
        }))
    }

    fn base_url(&self) -> &str {
        self.settings
            .base_url
            .as_deref()
            .unwrap_or(DEFAULT_BASE_URL)
    }
    fn keychain_service(&self) -> &str {
        self.settings
            .keychain_service
            .as_deref()
            .unwrap_or(DEFAULT_KEYCHAIN_SERVICE)
    }
    fn client_id(&self) -> &str {
        self.settings
            .client_id
            .as_deref()
            .unwrap_or(DEFAULT_CLIENT_ID)
    }
    fn token_url(&self) -> &str {
        self.settings
            .token_url
            .as_deref()
            .unwrap_or(DEFAULT_TOKEN_URL)
    }
    fn oauth_scopes(&self) -> &str {
        self.settings
            .oauth_scopes
            .as_deref()
            .unwrap_or(DEFAULT_OAUTH_SCOPES)
    }
    fn oauth_beta_header(&self) -> &str {
        self.settings
            .oauth_beta_header
            .as_deref()
            .unwrap_or(DEFAULT_OAUTH_BETA_HEADER)
    }
}

/// Validated reasoning knobs extracted from [`ClaudeCodeApiSettings`].
#[derive(Clone, Default)]
struct ReasoningConfig {
    /// `output_config.effort`: "max" | "high" | "medium" | "low".
    effort: Option<String>,
    /// `thinking.type`: "enabled" | "disabled" | "adaptive".
    thinking_mode: Option<String>,
    /// `thinking.budget_tokens`, only valid when mode=="enabled".
    thinking_budget: Option<usize>,
    /// Top-level sampling temperature. Forced to None when thinking is
    /// enabled (the API rejects temperature != 1.0 with thinking).
    temperature: Option<f64>,
    /// Overrides the LlmRequest::max_tokens when set.
    max_tokens_override: Option<usize>,
    /// When true, emit `tool_choice: { type: "auto",
    /// disable_parallel_tool_use: true }` on every request.
    strict_sequential: bool,
    /// Where to write the per-turn JSON dumps. None disables.
    dump_dir: Option<String>,
}

/// Validate the reasoning-related settings and coerce them into a
/// request-ready shape. Returns an error for invalid combinations so
/// misconfigured configs fail fast at load time instead of silently
/// producing broken API calls.
fn build_reasoning_config(
    settings: &ClaudeCodeApiSettings,
) -> Result<ReasoningConfig, PluginError> {
    if let Some(e) = settings.effort.as_deref() {
        let valid = ["max", "high", "medium", "low"];
        if !valid.contains(&e) {
            return Err(PluginError::new(format!(
                "claude-code-api: invalid effort '{e}' — expected one of: {}",
                valid.join(", ")
            )));
        }
    }

    if let Some(m) = settings.thinking_mode.as_deref() {
        let valid = ["enabled", "disabled", "adaptive"];
        if !valid.contains(&m) {
            return Err(PluginError::new(format!(
                "claude-code-api: invalid thinkingMode '{m}' — expected one of: {}",
                valid.join(", ")
            )));
        }
    }

    // thinking_budget without thinkingMode=enabled is meaningless on newer
    // models; permit the combo but warn.
    if let (Some(budget), Some(mode)) =
        (settings.thinking_budget, settings.thinking_mode.as_deref())
    {
        if mode != "enabled" && budget > 0 {
            eprintln!(
                "[claude-code-api] warning: thinkingBudget={budget} is ignored when thinkingMode='{mode}' (only 'enabled' uses it)"
            );
        }
    }

    #[allow(clippy::cast_sign_loss)]
    let thinking_budget = settings
        .thinking_budget
        .filter(|&b| b > 0)
        .map(|b| b as usize);

    #[allow(clippy::cast_sign_loss)]
    let max_tokens_override = settings.max_tokens.filter(|&m| m > 0).map(|m| m as usize);

    if let (Some(budget), Some(max_t)) = (thinking_budget, max_tokens_override) {
        if budget >= max_t {
            return Err(PluginError::new(format!(
                "claude-code-api: thinkingBudget ({budget}) must be less than maxTokens ({max_t})"
            )));
        }
    }

    // Temperature is incompatible with thinking (API requires 1.0)
    let thinking_will_run = matches!(
        settings.thinking_mode.as_deref(),
        Some("enabled" | "adaptive")
    );
    let temperature = if thinking_will_run {
        if let Some(t) = settings.temperature {
            if (t - 1.0).abs() > f64::EPSILON {
                eprintln!(
                    "[claude-code-api] warning: temperature={t} ignored — extended thinking requires temperature=1.0"
                );
            }
        }
        None
    } else {
        settings.temperature
    };

    // Resolution: settings.dumpDir → ADI_CLAUDE_DUMP_DIR env → default.
    // "off" disables.
    let dump_dir = match settings.dump_dir.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ => match std::env::var(DUMP_DIR_ENV).ok().as_deref().map(str::trim) {
            Some(s) if !s.is_empty() => Some(s.to_string()),
            _ => Some(default_dump_dir()),
        },
    };
    let dump_dir = dump_dir.filter(|s| s != "off");

    Ok(ReasoningConfig {
        effort: settings.effort.clone(),
        thinking_mode: settings.thinking_mode.clone(),
        thinking_budget,
        temperature,
        max_tokens_override,
        strict_sequential: settings.strict_sequential.unwrap_or(false),
        dump_dir,
    })
}

impl LoopRunnerPlugin for ClaudeCodeApi {
    fn kind(&self) -> &str {
        "claude-code-api"
    }

    fn resolve(&self, config: ConfigValue) -> Result<ConfigValue, PluginError> {
        if self.settings.model.is_empty() {
            return Err(PluginError::new("claude-code-api: 'model' is required"));
        }
        // Validate reasoning config at load time — this surfaces errors at
        // loop-init instead of on the first API call.
        build_reasoning_config(&self.settings)?;
        Ok(config)
    }

    fn build_backend(
        &self,
        _resolved_config: &ConfigValue,
    ) -> Result<Box<dyn LlmBackend>, PluginError> {
        let auth = resolve_auth(&self.settings, self.keychain_service())?;
        let reasoning = build_reasoning_config(&self.settings)?;

        Ok(Box::new(ClaudeCodeApiBackend {
            base_url: self.base_url().to_string(),
            model: self.settings.model.clone(),
            auth: Mutex::new(auth),
            client_id: self.client_id().to_string(),
            token_url: self.token_url().to_string(),
            oauth_scopes: self.oauth_scopes().to_string(),
            oauth_beta_header: self.oauth_beta_header().to_string(),
            reasoning,
            run_id: new_run_id(),
            seq: AtomicU64::new(1),
            summary_written: AtomicU64::new(0),
        }))
    }
}

// ── Auth ──

#[derive(Clone)]
enum Auth {
    ApiKey(String),
    OAuth(OAuthTokens),
}

#[derive(Clone)]
struct OAuthTokens {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
}

fn resolve_auth(
    settings: &ClaudeCodeApiSettings,
    keychain_service: &str,
) -> Result<Auth, PluginError> {
    if let Some(key) = &settings.api_key {
        if !key.is_empty() {
            return Ok(Auth::ApiKey(key.clone()));
        }
    }

    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            return Ok(Auth::ApiKey(key));
        }
    }

    if let Ok(token) = std::env::var("ANTHROPIC_OAUTH_TOKEN") {
        if !token.is_empty() {
            return Ok(Auth::OAuth(OAuthTokens {
                access_token: token,
                refresh_token: String::new(),
                expires_at: u64::MAX,
            }));
        }
    }

    read_keychain_oauth(keychain_service)
}

fn read_keychain_oauth(keychain_service: &str) -> Result<Auth, PluginError> {
    // The exact configured entry first. Claude Code ≥2.x keeps tokens in
    // per-config-dir entries (`Claude Code-credentials-<hash>`) and blanks
    // the legacy bare-name entry into a stub, so when the configured entry
    // is missing or a stub, scan its suffixed siblings and take the
    // freshest — the one a live Claude Code session keeps refreshed.
    if let Some(tokens) = keychain_entry_tokens(keychain_service)? {
        return Ok(Auth::OAuth(tokens));
    }

    let mut best: Option<OAuthTokens> = None;
    for svc in list_credential_services(keychain_service) {
        if let Ok(Some(tokens)) = keychain_entry_tokens(&svc) {
            if best
                .as_ref()
                .is_none_or(|b| tokens.expires_at > b.expires_at)
            {
                best = Some(tokens);
            }
        }
    }
    best.map(Auth::OAuth).ok_or_else(|| {
        PluginError::new(
            "claude-code-api: no API key and no OAuth tokens found in keychain. \
             Set 'apiKey' in config or log in via `claude` CLI first.",
        )
    })
}

/// The OAuth tokens stored under one keychain service, or `None` when the
/// entry is missing, unparsable, or a stub with an empty access token.
fn keychain_entry_tokens(service: &str) -> Result<Option<OAuthTokens>, PluginError> {
    let user = std::env::var("USER").unwrap_or_else(|_| "claude-code-user".to_string());

    let output = Command::new("security")
        .args(["find-generic-password", "-a", &user, "-s", service, "-w"])
        .output()
        .map_err(|e| PluginError::new(format!("keychain read failed: {e}")))?;
    if !output.status.success() {
        return Ok(None);
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let Ok(creds) = serde_json::from_str::<serde_json::Value>(json_str.trim()) else {
        return Ok(None);
    };
    let Some(oauth) = creds.get("claudeAiOauth") else {
        return Ok(None);
    };

    let token = |key: &str| {
        oauth
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
    };
    let Some(access_token) = token("accessToken") else {
        return Ok(None);
    };
    Ok(Some(OAuthTokens {
        access_token,
        refresh_token: token("refreshToken").unwrap_or_default(),
        expires_at: oauth
            .get("expiresAt")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    }))
}

/// Suffixed sibling entries of `base` in the keychain (`<base>-<hex>`),
/// found by scanning `security dump-keychain` attribute lines. Errors and
/// odd output read as "none" — the caller falls through to its own error.
fn list_credential_services(base: &str) -> Vec<String> {
    let Ok(output) = Command::new("security").arg("dump-keychain").output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let prefix = format!("\"svce\"<blob>=\"{base}-");
    let mut services: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix(&prefix)?;
            let suffix = rest.strip_suffix('"')?;
            (!suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_hexdigit()))
                .then(|| format!("{base}-{suffix}"))
        })
        .collect();
    services.sort();
    services.dedup();
    services
}

fn refresh_oauth(
    tokens: &OAuthTokens,
    client_id: &str,
    token_url: &str,
    oauth_scopes: &str,
) -> Result<OAuthTokens, PluginError> {
    if tokens.refresh_token.is_empty() {
        return Err(PluginError::new(
            "claude-code-api: OAuth token expired and no refresh token available — \
             log in via `claude` first",
        ));
    }
    eprintln!("[claude-code-api] refreshing OAuth token");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| PluginError::new(format!("tokio runtime: {e}")))?;

    rt.block_on(async {
        let body = serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": tokens.refresh_token,
            "client_id": client_id,
            "scope": oauth_scopes,
        });

        let resp = reqwest::Client::new()
            .post(token_url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| PluginError::new(format!("token refresh request: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            return Err(PluginError::new(format!(
                "token refresh failed ({status}): {detail}"
            )));
        }

        let data: TokenRefreshResponse = resp
            .json()
            .await
            .map_err(|e| PluginError::new(format!("token refresh parse: {e}")))?;

        Ok(OAuthTokens {
            access_token: data.access_token,
            refresh_token: data
                .refresh_token
                .unwrap_or_else(|| tokens.refresh_token.clone()),
            expires_at: now_unix_ms() + (data.expires_in * 1000),
        })
    })
}

fn now_unix_ms() -> u64 {
    #[allow(clippy::cast_possible_truncation)]
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn is_token_expired(tokens: &OAuthTokens) -> bool {
    now_unix_ms() + 60_000 >= tokens.expires_at
}

#[derive(Deserialize)]
struct TokenRefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

// ── API types ──

#[derive(Serialize, Clone)]
struct CacheControl {
    #[serde(rename = "type")]
    kind: &'static str,
}

impl CacheControl {
    const fn ephemeral() -> Self {
        Self { kind: "ephemeral" }
    }
}

#[derive(Serialize)]
struct SystemBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: usize,
    stream: bool,
    system: Vec<SystemBlock>,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    /// `thinking: { type: "enabled" | "adaptive" | "disabled", budget_tokens? }`
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
    /// `output_config: { effort: "max" | "high" | "medium" | "low" }`
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<OutputConfig>,
}

/// Anthropic `tool_choice` lets us force / disable tool use. We only
/// use this to opt into `disable_parallel_tool_use=true` for loops
/// where tool-call ordering matters.
#[derive(Debug, Clone, Serialize)]
struct ToolChoice {
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    disable_parallel_tool_use: Option<bool>,
}

/// Extended thinking configuration. The `budget_tokens` field is only
/// honored when `mode == "enabled"`; adaptive/disabled ignore it.
#[derive(Serialize)]
struct ThinkingConfig {
    #[serde(rename = "type")]
    mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_tokens: Option<usize>,
}

/// Response-shape control per the Anthropic API.
#[derive(Serialize)]
struct OutputConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
}

#[derive(Serialize)]
struct ApiTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: Vec<ContentBlock>,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// Thinking block echoed back to the API on subsequent turns.
    /// Anthropic validates the signature so we MUST preserve it verbatim.
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// Redacted thinking echoed back. Carries opaque data the provider
    /// stripped for safety; must also be preserved if present.
    #[serde(rename = "redacted_thinking")]
    RedactedThinking {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

impl ContentBlock {
    /// Attach an ephemeral cache breakpoint to this block.
    fn set_cache_control(&mut self, cc: CacheControl) {
        match self {
            ContentBlock::Text { cache_control, .. }
            | ContentBlock::ToolUse { cache_control, .. }
            | ContentBlock::ToolResult { cache_control, .. }
            | ContentBlock::Thinking { cache_control, .. }
            | ContentBlock::RedactedThinking { cache_control, .. } => *cache_control = Some(cc),
        }
    }
}

#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<ResponseBlock>,
    usage: ApiUsage,
}

#[derive(Deserialize)]
struct ApiUsage {
    input_tokens: usize,
    output_tokens: usize,
    #[serde(default)]
    cache_creation_input_tokens: usize,
    #[serde(default)]
    cache_read_input_tokens: usize,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ResponseBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Extended thinking block; preserved so it can be echoed back on
    /// subsequent tool-use turns (Anthropic validates the signature).
    #[serde(rename = "thinking")]
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },
    /// Redacted thinking (provider stripped content for safety).
    #[serde(rename = "redacted_thinking")]
    RedactedThinking {
        #[serde(default)]
        data: Option<String>,
    },
}

// ── Conversions ──

/// Build the Anthropic `messages` array from our `Turn` schema. Each Turn
/// maps 1:1 to an ApiMessage; block order is critical for interleaved
/// thinking (signatures are position-scoped).
fn convert_turns(turns: &[Turn]) -> Vec<ApiMessage> {
    let mut result: Vec<ApiMessage> = Vec::with_capacity(turns.len());

    for turn in turns {
        match turn {
            Turn::User(ut) => {
                let content: Vec<ContentBlock> = ut
                    .blocks
                    .iter()
                    .map(|b| match b {
                        UserBlock::Text(text) => ContentBlock::Text {
                            text: text.clone(),
                            cache_control: None,
                        },
                        UserBlock::ToolResult {
                            tool_use_id,
                            content,
                        } => ContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: content.clone(),
                            cache_control: None,
                        },
                    })
                    .collect();
                // A turn must have at least one block; if an empty user
                // turn somehow arrives, emit an empty text block so
                // Anthropic doesn't reject the request.
                let content = if content.is_empty() {
                    vec![ContentBlock::Text {
                        text: String::new(),
                        cache_control: None,
                    }]
                } else {
                    content
                };
                result.push(ApiMessage {
                    role: "user".to_string(),
                    content,
                });
            }
            Turn::Assistant(at) => {
                let content: Vec<ContentBlock> = at
                    .blocks
                    .iter()
                    .map(|b| match b {
                        AssistantBlock::Thinking {
                            text,
                            signature,
                            redacted,
                        } => {
                            if *redacted {
                                ContentBlock::RedactedThinking {
                                    data: Some(text.clone()),
                                    cache_control: None,
                                }
                            } else {
                                ContentBlock::Thinking {
                                    thinking: text.clone(),
                                    signature: signature.clone(),
                                    cache_control: None,
                                }
                            }
                        }
                        AssistantBlock::Text(text) => ContentBlock::Text {
                            text: text.clone(),
                            cache_control: None,
                        },
                        AssistantBlock::ToolUse {
                            id,
                            name,
                            arguments,
                        } => ContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: serde_json::from_str(arguments)
                                .unwrap_or(serde_json::Value::Object(serde_json::Map::default())),
                            cache_control: None,
                        },
                    })
                    .collect();
                let content = if content.is_empty() {
                    vec![ContentBlock::Text {
                        text: String::new(),
                        cache_control: None,
                    }]
                } else {
                    content
                };
                result.push(ApiMessage {
                    role: "assistant".to_string(),
                    content,
                });
            }
        }
    }

    result
}

fn convert_tools(tools: &[ToolDef]) -> Vec<ApiTool> {
    tools
        .iter()
        .map(|t| ApiTool {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: serde_json::from_str(&t.parameters_json)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::default())),
            cache_control: None,
        })
        .collect()
}

/// Place ephemeral cache breakpoints for maximum prompt-caching effect:
/// last tool, last system block, second-to-last message's last block, and
/// last message's last block (up to Anthropic's 4-breakpoint limit).
/// Breakpoints only control cache CREATION — the server matches the
/// longest cached prefix regardless.
fn apply_cache_breakpoints(
    tools: &mut [ApiTool],
    system: &mut [SystemBlock],
    messages: &mut [ApiMessage],
) {
    if let Some(last_tool) = tools.last_mut() {
        last_tool.cache_control = Some(CacheControl::ephemeral());
    }

    if let Some(last_sys) = system.last_mut() {
        last_sys.cache_control = Some(CacheControl::ephemeral());
    }

    if messages.len() >= 2 {
        let idx = messages.len() - 2;
        if let Some(last_block) = messages[idx].content.last_mut() {
            last_block.set_cache_control(CacheControl::ephemeral());
        }
    }

    if let Some(last_msg) = messages.last_mut() {
        if let Some(last_block) = last_msg.content.last_mut() {
            last_block.set_cache_control(CacheControl::ephemeral());
        }
    }
}

fn parse_response(resp: ApiResponse, model: &str) -> LlmResponse {
    // Map each response block 1:1 to an AssistantBlock, preserving order —
    // thinking signatures are position-scoped.
    let mut blocks: Vec<AssistantBlock> = Vec::with_capacity(resp.content.len());

    for block in resp.content {
        match block {
            ResponseBlock::Text { text } => {
                blocks.push(AssistantBlock::Text(text));
            }
            ResponseBlock::ToolUse { id, name, input } => {
                blocks.push(AssistantBlock::ToolUse {
                    id,
                    name,
                    arguments: serde_json::to_string(&input).unwrap_or_default(),
                });
            }
            ResponseBlock::Thinking {
                thinking,
                signature,
            } => {
                blocks.push(AssistantBlock::Thinking {
                    text: thinking,
                    signature,
                    redacted: false,
                });
            }
            ResponseBlock::RedactedThinking { data } => {
                blocks.push(AssistantBlock::Thinking {
                    text: data.unwrap_or_default(),
                    signature: None,
                    redacted: true,
                });
            }
        }
    }

    LlmResponse {
        turn: AssistantTurn { blocks },
        model: model.to_string(),
        input_tokens: resp.usage.input_tokens,
        output_tokens: resp.usage.output_tokens,
        cache_creation_input_tokens: resp.usage.cache_creation_input_tokens,
        cache_read_input_tokens: resp.usage.cache_read_input_tokens,
    }
}

// ── Backend ──

struct ClaudeCodeApiBackend {
    base_url: String,
    model: String,
    auth: Mutex<Auth>,
    client_id: String,
    token_url: String,
    oauth_scopes: String,
    oauth_beta_header: String,
    reasoning: ReasoningConfig,
    /// Stable per-loop folder name. Each `build_backend()` (i.e. each
    /// loop start) gets a fresh id so concurrent loops don't collide.
    run_id: String,
    /// Per-backend monotonic counter; zero-padded in the filename so
    /// each loop folder reads cleanly: 000001, 000002, ...
    seq: AtomicU64,
    /// One-shot CAS flag for `summary.json` — written on first call only.
    summary_written: AtomicU64,
}

impl ClaudeCodeApiBackend {
    fn get_auth_headers(&self) -> Result<Vec<(&'static str, String)>, PluginError> {
        let mut auth = self.auth.lock().unwrap();

        match &mut *auth {
            Auth::ApiKey(key) => Ok(vec![("x-api-key", key.clone())]),
            Auth::OAuth(tokens) => {
                if is_token_expired(tokens) {
                    *tokens = refresh_oauth(
                        tokens,
                        &self.client_id,
                        &self.token_url,
                        &self.oauth_scopes,
                    )?;
                }
                Ok(vec![
                    ("Authorization", format!("Bearer {}", tokens.access_token)),
                    ("anthropic-beta", self.oauth_beta_header.clone()),
                ])
            }
        }
    }
}

impl LlmBackend for ClaudeCodeApiBackend {
    #[allow(clippy::too_many_lines)]
    fn call(&self, request: &LlmRequest) -> Result<LlmResponse, PluginError> {
        let is_oauth = matches!(*self.auth.lock().unwrap(), Auth::OAuth(_));
        let url = if is_oauth {
            format!("{}/v1/messages?beta=true", self.base_url)
        } else {
            format!("{}/v1/messages", self.base_url)
        };
        let auth_headers = self.get_auth_headers()?;

        let mut api_tools = convert_tools(&request.tools);
        let mut system = vec![
            SystemBlock {
                block_type: "text".to_string(),
                text: format!("x-anthropic-billing-header: cc_version={CLI_VERSION}.unknown; cc_entrypoint=cli; cch=00000;"),
                cache_control: None,
            },
            SystemBlock {
                block_type: "text".to_string(),
                text: format!("You are Claude Code, Anthropic's official CLI for Claude.\n{}", request.system_prompt),
                cache_control: None,
            },
        ];
        let mut messages = convert_turns(&request.turns);

        apply_cache_breakpoints(&mut api_tools, &mut system, &mut messages);

        let max_tokens = self
            .reasoning
            .max_tokens_override
            .unwrap_or(request.max_tokens);

        let thinking = self
            .reasoning
            .thinking_mode
            .as_deref()
            .map(|mode| ThinkingConfig {
                mode: mode.to_string(),
                budget_tokens: if mode == "enabled" {
                    self.reasoning.thinking_budget
                } else {
                    None
                },
            });

        let output_config = self.reasoning.effort.as_deref().map(|e| OutputConfig {
            effort: Some(e.to_string()),
        });

        let tool_choice = if self.reasoning.strict_sequential && !api_tools.is_empty() {
            Some(ToolChoice {
                kind: "auto".to_string(),
                disable_parallel_tool_use: Some(true),
            })
        } else {
            None
        };

        let body = ApiRequest {
            model: self.model.clone(),
            max_tokens,
            stream: false,
            system,
            messages,
            tools: api_tools,
            tool_choice,
            temperature: self.reasoning.temperature,
            thinking,
            output_config,
        };

        // ── Dump request body (always-on observability) ─
        let request_bytes = serde_json::to_vec(&body).map(|v| v.len()).unwrap_or(0);
        let dump_target = self.reasoning.dump_dir.as_deref().map(|root| {
            let dir = format!("{root}/{}", self.run_id);
            let n = self.seq.fetch_add(1, Ordering::Relaxed);
            let stem = format!("{n:06}");
            if self
                .summary_written
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let summary = BackendSummary {
                    run_id: self.run_id.clone(),
                    model: self.model.clone(),
                    base_url: self.base_url.clone(),
                    effort: self.reasoning.effort.clone(),
                    thinking_mode: self.reasoning.thinking_mode.clone(),
                    thinking_budget: self.reasoning.thinking_budget,
                    strict_sequential: self.reasoning.strict_sequential,
                    max_tokens_override: self.reasoning.max_tokens_override,
                    started_at_unix_ms: now_unix_ms(),
                    first_system_prompt: request.system_prompt.clone(),
                    first_tools: request.tools.iter().map(|t| t.name.clone()).collect(),
                };
                if let Err(e) = write_dump(&dir, "", "summary.json", &summary) {
                    eprintln!(
                        "[claude-dump] WARN: summary write failed run_id={}: {e}",
                        self.run_id
                    );
                }
            }
            if let Err(e) = write_dump(&dir, &stem, "request.json", &body) {
                eprintln!(
                    "[claude-dump] WARN: request dump failed run_id={} seq={stem}: {e}",
                    self.run_id
                );
            }
            eprintln!(
                "[claude-dump] run={} seq={stem} request_bytes={request_bytes} model={}",
                self.run_id, self.model,
            );
            (dir, stem)
        });
        let dump_t0 = std::time::Instant::now();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| PluginError::new(format!("tokio runtime: {e}")))?;

        rt.block_on(async {
            let client = reqwest::Client::new();
            let max_retries = 5u32;

            for attempt in 0..=max_retries {
                let session_id = uuid_v4();
                let mut req = client
                    .post(&url)
                    .header("anthropic-version", "2023-06-01")
                    .header("content-type", "application/json")
                    .header("user-agent", format!("claude-cli/{CLI_VERSION} (external, cli)"))
                    .header("x-app", "cli")
                    .header("x-claude-code-session-id", &session_id)
                    .header("x-client-request-id", uuid_v4())
                    .header("X-Stainless-Lang", "js")
                    .header("X-Stainless-Package-Version", SDK_VERSION)
                    .header("X-Stainless-Runtime", "node")
                    .header("X-Stainless-Runtime-Version", "v22.14.0")
                    .header("X-Stainless-OS", "MacOS")
                    .header("X-Stainless-Arch", std::env::consts::ARCH)
                    .header("X-Stainless-Retry-Count", "0")
                    .timeout(std::time::Duration::from_secs(600));

                for (key, value) in &auth_headers {
                    req = req.header(*key, value);
                }

                let resp = req.json(&body).send().await
                    .map_err(|e| PluginError::new(format!("claude-code-api request: {e}")))?;

                let status = resp.status();
                // Snapshot headers BEFORE consuming the body for the meta dump.
                let dump_headers: Vec<(String, String)> = if dump_target.is_some() {
                    resp.headers()
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
                        .collect()
                } else {
                    Vec::new()
                };

                if status == 429 || status.as_u16() == 529 {
                    if attempt < max_retries {
                        let retry_after = resp.headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.parse::<u64>().ok())
                            .unwrap_or(2u64.pow(attempt).min(30));
                        eprintln!("[claude-code-api] rate limited, retrying in {retry_after}s (attempt {}/{})", attempt + 1, max_retries);
                        tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;
                        continue;
                    }
                    let detail = resp.text().await.unwrap_or_default();
                    return Err(PluginError::new(format!("claude-code-api rate limited after {max_retries} retries: {detail}")));
                }

                if !status.is_success() {
                    let detail = resp.text().await.unwrap_or_default();
                    if let Some((dir, stem)) = dump_target.as_ref() {
                        #[allow(clippy::cast_possible_truncation)]
                        let elapsed_ms = dump_t0.elapsed().as_millis() as u64;
                        let _ = write_dump_text(dir, stem, "response.json", &detail);
                        let meta = DumpMeta {
                            run_id: self.run_id.clone(),
                            seq: stem.clone(),
                            status: status.as_u16(),
                            elapsed_ms,
                            request_bytes,
                            response_bytes: detail.len(),
                            ok: false,
                            input_tokens: 0,
                            output_tokens: 0,
                            cache_creation_input_tokens: 0,
                            cache_read_input_tokens: 0,
                            response_headers: dump_headers,
                        };
                        let _ = write_dump(dir, stem, "meta.json", &meta);
                    }
                    if status == 401 {
                        return Err(PluginError::new(format!("claude-code-api auth error ({status}): {detail}")));
                    }
                    return Err(PluginError::new(format!("claude-code-api error ({status}): {detail}")));
                }

                // Consume body as text first so we always dump the raw bytes
                // even if JSON parsing fails downstream.
                let body_text = resp.text().await
                    .map_err(|e| PluginError::new(format!("claude-code-api body: {e}")))?;
                if let Some((dir, stem)) = dump_target.as_ref() {
                    let _ = write_dump_text(dir, stem, "response.json", &body_text);
                }
                let api_resp: ApiResponse = serde_json::from_str(&body_text)
                    .map_err(|e| PluginError::new(format!("claude-code-api parse: {e}")))?;
                let parsed = parse_response(api_resp, &self.model);
                if let Some((dir, stem)) = dump_target.as_ref() {
                    #[allow(clippy::cast_possible_truncation)]
                    let elapsed_ms = dump_t0.elapsed().as_millis() as u64;
                    let meta = DumpMeta {
                        run_id: self.run_id.clone(),
                        seq: stem.clone(),
                        status: status.as_u16(),
                        elapsed_ms,
                        request_bytes,
                        response_bytes: body_text.len(),
                        ok: true,
                        input_tokens: parsed.input_tokens,
                        output_tokens: parsed.output_tokens,
                        cache_creation_input_tokens: parsed.cache_creation_input_tokens,
                        cache_read_input_tokens: parsed.cache_read_input_tokens,
                        response_headers: dump_headers,
                    };
                    let _ = write_dump(dir, stem, "meta.json", &meta);
                    eprintln!(
                        "[claude-dump] run={} seq={stem} status={} elapsed_ms={elapsed_ms} resp_bytes={} in_tok={} out_tok={} cache_w={} cache_r={}",
                        self.run_id, status.as_u16(), body_text.len(), parsed.input_tokens, parsed.output_tokens, parsed.cache_creation_input_tokens, parsed.cache_read_input_tokens,
                    );
                }
                return Ok(parsed);
            }

            Err(PluginError::new("claude-code-api: exhausted retries".to_string()))
        })
    }
}

// ── Dump helpers ──

fn new_run_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let now_ms = now_unix_ms();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{now_ms}-{}-{n:04}", std::process::id())
}

fn write_dump<T: Serialize>(dir: &str, stem: &str, suffix: &str, value: &T) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let filename = if stem.is_empty() {
        suffix.to_string()
    } else {
        format!("{stem}-{suffix}")
    };
    let path = std::path::Path::new(dir).join(filename);
    let pretty = serde_json::to_vec_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, pretty)
}

fn write_dump_text(dir: &str, stem: &str, suffix: &str, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let filename = if stem.is_empty() {
        suffix.to_string()
    } else {
        format!("{stem}-{suffix}")
    };
    let path = std::path::Path::new(dir).join(filename);
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        let pretty = serde_json::to_vec_pretty(&v)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, pretty)
    } else {
        std::fs::write(path, text)
    }
}

#[derive(Serialize)]
struct BackendSummary {
    run_id: String,
    model: String,
    base_url: String,
    effort: Option<String>,
    thinking_mode: Option<String>,
    thinking_budget: Option<usize>,
    strict_sequential: bool,
    max_tokens_override: Option<usize>,
    started_at_unix_ms: u64,
    first_system_prompt: String,
    first_tools: Vec<String>,
}

#[derive(Serialize)]
struct DumpMeta {
    run_id: String,
    seq: String,
    status: u16,
    elapsed_ms: u64,
    request_bytes: usize,
    response_bytes: usize,
    ok: bool,
    /// Anthropic's non-cached prompt tokens (billed at full input rate).
    input_tokens: usize,
    output_tokens: usize,
    /// Tokens written to the prompt cache on this request (~1.25× input rate).
    cache_creation_input_tokens: usize,
    /// Tokens read from the prompt cache (~0.1× input rate).
    cache_read_input_tokens: usize,
    response_headers: Vec<(String, String)>,
}
