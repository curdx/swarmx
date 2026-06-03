//! MCP admin — 给「快捷装 MCP」页面用的后端。
//!
//! 三件事，全部 loopback 单用户、无 auth(同本仓库其它 REST):
//!   1. `GET  /api/mcp/env`        探运行时(node/npm/uv)版本+是否存在 —— npx 类
//!                                 MCP 缺 node 跑不起来，页面要警告。
//!   2. `GET  /api/mcp/status`     读用户**现有**已配置的 MCP(claude 读
//!                                 `~/.claude.json` user-scope、codex 读
//!                                 `~/.codex/config.toml`)，按 CLI 分别返回名字。
//!   3. `POST /api/mcp/install` / `/uninstall`  调 CLI 自己的 `mcp add/remove`
//!                                 子命令真装真卸 —— 装了立刻对 spawn 的 agent 生效。
//!
//! 安全：server 名做字符校验；**可装的 server 来自服务端 allowlist**(不执行前端
//! 传来的任意命令)。先只放 chrome-devtools 一个(用户要求 "先只加这一个")。

use crate::AppState;
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::process::Command;

// ── 运行时探测 ──────────────────────────────────────────────────────

async fn probe_version(bin: &str, arg: &str) -> Option<String> {
    let out = Command::new(bin).arg(arg).output().await.ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // Some tools echo their own name (`uv --version` → "uv 0.9.2 (…)"). The UI
    // shows the name as a chip label already, so strip a leading "<bin> " to
    // avoid rendering "uv uv 0.9.2". node/npm print a bare version → untouched.
    let s = s.strip_prefix(&format!("{bin} ")).map(str::to_string).unwrap_or(s);
    if !s.is_empty() {
        return Some(s);
    }
    let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if e.is_empty() {
        None
    } else {
        Some(e)
    }
}

pub async fn mcp_env(State(_s): State<AppState>) -> impl IntoResponse {
    // node/npm 用 `--version`；uv 同样。并行探，少等几百毫秒。
    let (node, npm, uv) = tokio::join!(
        probe_version("node", "--version"),
        probe_version("npm", "--version"),
        probe_version("uv", "--version"),
    );
    Json(json!({
        "node": { "present": node.is_some(), "version": node },
        "npm":  { "present": npm.is_some(),  "version": npm },
        "uv":   { "present": uv.is_some(),   "version": uv },
    }))
}

// ── 可装的 server allowlist(服务端,防任意命令执行) ─────────────────
// 加新 server = 在 `known` 加一条。命令出处见各自官方 README/docs。

/// 一个可装 server 的元信息。
struct Known {
    command: &'static str,
    args: &'static [&'static str],
    /// 需要 API key 时，key 以这个 flag 追加在 args 末尾(如 "--api-key")；
    /// None = 无需 key。
    api_key_flag: Option<&'static str>,
}

fn known(name: &str) -> Option<Known> {
    match name {
        // chrome-devtools README: `npx chrome-devtools-mcp@latest`，无 key。
        "chrome-devtools" => Some(Known {
            command: "npx",
            args: &["-y", "chrome-devtools-mcp@latest"],
            api_key_flag: None,
        }),
        // context7 all-clients docs: `npx -y @upstash/context7-mcp --api-key KEY`。
        "context7" => Some(Known {
            command: "npx",
            args: &["-y", "@upstash/context7-mcp"],
            api_key_flag: Some("--api-key"),
        }),
        _ => None,
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// ── 读现有配置 ──────────────────────────────────────────────────────

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// `~/.claude.json` 顶层(user scope)`mcpServers` 的名字。
fn claude_servers() -> Vec<String> {
    let Some(h) = home() else {
        return vec![];
    };
    let Ok(txt) = std::fs::read_to_string(h.join(".claude.json")) else {
        return vec![];
    };
    let Ok(v) = serde_json::from_str::<Value>(&txt) else {
        return vec![];
    };
    v.get("mcpServers")
        .and_then(|m| m.as_object())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// `~/.codex/config.toml` 里的 `[mcp_servers.<name>]` 段名。
fn codex_servers() -> Vec<String> {
    let Some(h) = home() else {
        return vec![];
    };
    let Ok(txt) = std::fs::read_to_string(h.join(".codex").join("config.toml")) else {
        return vec![];
    };
    let mut out = vec![];
    for line in txt.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("[mcp_servers.") {
            if let Some(name) = rest.strip_suffix(']') {
                out.push(name.trim().trim_matches('"').to_string());
            }
        }
    }
    out
}

/// 需密钥 server 的 key 状态(给前端：是否已设置 / 打码回显 / 两个 CLI 是否一致)。
fn key_state(name: &str) -> Value {
    let ck = claude_key(name);
    let xk = codex_key(name);
    let any = ck.as_deref().or(xk.as_deref());
    let consistent = match (&ck, &xk) {
        (Some(a), Some(b)) => a == b,
        _ => true,
    };
    json!({
        "present": any.is_some(),
        "masked": any.map(mask_key),
        "consistent": consistent,
    })
}

pub async fn mcp_status(State(_s): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "claude": claude_servers(),
        "codex": codex_servers(),
        // 需密钥 server 的 key 状态(目前只有 context7)。
        "keys": { "context7": key_state("context7") },
    }))
}

// ── key 复用：claude / codex 共用同一把 key ──────────────────────────
// 装需密钥的 server 时若前端没传 key，就从「已经配过这个 server 的那个 CLI」
// 的配置里把 `--api-key <key>` 取出来复用 —— 用户只需填一次。

fn extract_key_after_flag(tokens: &[String]) -> Option<String> {
    for (i, t) in tokens.iter().enumerate() {
        if t == "--api-key" {
            return tokens.get(i + 1).cloned().filter(|s| !s.is_empty());
        }
        if let Some(v) = t.strip_prefix("--api-key=") {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// claude ~/.claude.json → mcpServers[name].args 里的 --api-key 值。
fn claude_key(name: &str) -> Option<String> {
    let h = home()?;
    let txt = std::fs::read_to_string(h.join(".claude.json")).ok()?;
    let v: Value = serde_json::from_str(&txt).ok()?;
    let args = v
        .get("mcpServers")
        .and_then(|m| m.get(name))
        .and_then(|s| s.get("args"))
        .and_then(|a| a.as_array())?;
    let toks: Vec<String> = args.iter().filter_map(|a| a.as_str().map(String::from)).collect();
    extract_key_after_flag(&toks)
}

/// codex ~/.codex/config.toml → [mcp_servers.<name>] 段 args 行里的 --api-key 值。
fn codex_key(name: &str) -> Option<String> {
    let h = home()?;
    let txt = std::fs::read_to_string(h.join(".codex").join("config.toml")).ok()?;
    let header = format!("[mcp_servers.{name}]");
    let mut in_section = false;
    for line in txt.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_section = l == header;
            continue;
        }
        if in_section && l.contains("--api-key") {
            // 按引号切：`"--api-key"` 后的下一段引号内内容(i+2)即 key 值。
            let toks: Vec<String> = l.split('"').map(String::from).collect();
            for (i, tk) in toks.iter().enumerate() {
                if tk == "--api-key" {
                    if let Some(v) = toks.get(i + 2) {
                        if !v.is_empty() {
                            return Some(v.clone());
                        }
                    }
                }
            }
        }
    }
    None
}

/// 任一 CLI 已配的 key —— 装到另一个 CLI 时复用(claude/codex 共用一把 key)。
fn recover_api_key(name: &str) -> Option<String> {
    claude_key(name).or_else(|| codex_key(name))
}

/// 末 4 位以外打码，用于回显「已设置」状态而不泄露完整 key。
fn mask_key(k: &str) -> String {
    let n = k.chars().count();
    if n <= 4 {
        "••••".to_string()
    } else {
        format!("••••{}", &k[k.len().saturating_sub(4)..])
    }
}

// ── 装 / 卸：调 CLI 自己的 `mcp` 子命令 ──────────────────────────────

#[derive(Deserialize)]
pub struct McpMutate {
    pub name: String,
    pub cli: String,
    /// 仅装需要密钥的 server 时带(如 context7)。卸载不需要。
    #[serde(default)]
    pub api_key: Option<String>,
}

async fn run<S: AsRef<std::ffi::OsStr>>(
    bin: &str,
    args: &[S],
) -> Result<String, (StatusCode, String)> {
    let out = Command::new(bin).args(args).output().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{bin} 启动失败: {e}（是否已安装并在 PATH 中？）"),
        )
    })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err((
            StatusCode::BAD_GATEWAY,
            if err.is_empty() {
                format!("{bin} 退出码非零")
            } else {
                err
            },
        ))
    }
}

pub async fn mcp_install(
    State(_s): State<AppState>,
    Json(req): Json<McpMutate>,
) -> impl IntoResponse {
    if !valid_name(&req.name) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "非法 server 名"}))).into_response();
    }
    let Some(k) = known(&req.name) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("未知 server: {}", req.name)})),
        )
            .into_response();
    };
    // server 命令的 args，需要 key 时把 `--api-key <key>` 追加在末尾。
    let mut svc_args: Vec<String> = k.args.iter().map(|s| s.to_string()).collect();
    if let Some(flag) = k.api_key_flag {
        let mut key = req.api_key.as_deref().unwrap_or("").trim().to_string();
        if key.is_empty() {
            // claude / codex 共用一把 key：前端没传就复用另一处已配的(用户只填一次)。
            key = recover_api_key(&req.name).unwrap_or_default();
        }
        if key.is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("{} 需要 API key", req.name)})),
            )
                .into_response();
        }
        svc_args.push(flag.to_string());
        svc_args.push(key);
    }
    // claude: `mcp add <name> --scope user -- <cmd> <args...>`
    // codex:  `mcp add <name> -- <cmd> <args...>`
    let (bin, mut a): (&str, Vec<String>) = match req.cli.as_str() {
        "claude" => (
            "claude",
            vec![
                "mcp".into(),
                "add".into(),
                req.name.clone(),
                "--scope".into(),
                "user".into(),
                "--".into(),
                k.command.into(),
            ],
        ),
        "codex" => (
            "codex",
            vec![
                "mcp".into(),
                "add".into(),
                req.name.clone(),
                "--".into(),
                k.command.into(),
            ],
        ),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "未知 CLI"}))).into_response()
        }
    };
    a.extend(svc_args);
    // upsert：先 remove(忽略「不存在」的失败)，让「改 key / 重装」幂等 —— 不会
    // 撞上 `mcp add` 对已存在 name 的报错，也保证改 key 时旧条目被新值覆盖。
    let remove_args: Vec<String> = if bin == "claude" {
        vec![
            "mcp".into(),
            "remove".into(),
            req.name.clone(),
            "--scope".into(),
            "user".into(),
        ]
    } else {
        vec!["mcp".into(), "remove".into(), req.name.clone()]
    };
    let _ = run(bin, &remove_args).await;
    let res = run(bin, &a).await;
    match res {
        Ok(output) => (StatusCode::OK, Json(json!({"ok": true, "output": output}))).into_response(),
        Err((code, msg)) => (code, Json(json!({"error": msg}))).into_response(),
    }
}

pub async fn mcp_uninstall(
    State(_s): State<AppState>,
    Json(req): Json<McpMutate>,
) -> impl IntoResponse {
    if !valid_name(&req.name) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "非法 server 名"}))).into_response();
    }
    let res = match req.cli.as_str() {
        "claude" => run("claude", &["mcp", "remove", &req.name, "--scope", "user"]).await,
        "codex" => run("codex", &["mcp", "remove", &req.name]).await,
        _ => Err((StatusCode::BAD_REQUEST, "未知 CLI".to_string())),
    };
    match res {
        Ok(output) => (StatusCode::OK, Json(json!({"ok": true, "output": output}))).into_response(),
        Err((code, msg)) => (code, Json(json!({"error": msg}))).into_response(),
    }
}
