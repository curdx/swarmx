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

// ── 运行时探测 ──────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct VersionProbe {
    version: String,
    path: Option<String>,
}

fn resolved_program(bin: &str) -> (PathBuf, Option<String>) {
    match crate::runtime_path::resolve_executable(bin) {
        Some(path) => {
            let display = path.to_string_lossy().into_owned();
            (path, Some(display))
        }
        None => (PathBuf::from(bin), None),
    }
}

async fn probe_version(bin: &str, arg: &str) -> Option<VersionProbe> {
    let (program, path) = resolved_program(bin);
    let out = crate::runtime_path::tool_command_async(&program)
        .arg(arg)
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // Some tools echo their own name (`uv --version` → "uv 0.9.2 (…)"). The UI
    // shows the name as a chip label already, so strip a leading "<bin> " to
    // avoid rendering "uv uv 0.9.2". node/npm print a bare version → untouched.
    let s = s
        .strip_prefix(&format!("{bin} "))
        .map(str::to_string)
        .unwrap_or(s);
    if !s.is_empty() {
        return Some(VersionProbe { version: s, path });
    }
    let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if e.is_empty() {
        None
    } else {
        Some(VersionProbe { version: e, path })
    }
}

/// Minimum Node major the npx-based MCP servers (chrome-devtools, context7…)
/// need. They target current LTS; anything below this can't run them — so a
/// merely-present-but-too-old node must NOT read as "OK" in the UI.
const NODE_MIN_MAJOR: u32 = 18;

/// Parse the major version out of a `node --version` string ("v22.17.0" → 22).
fn node_major(version: &str) -> Option<u32> {
    version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .next()?
        .parse()
        .ok()
}

pub async fn mcp_env(State(_s): State<AppState>) -> impl IntoResponse {
    // node/npm 用 `--version`；uv 同样。并行探，少等几百毫秒。
    let (node, npm, uv) = tokio::join!(
        probe_version("node", "--version"),
        probe_version("npm", "--version"),
        probe_version("uv", "--version"),
    );
    // `adequate` = present AND major >= LTS minimum. A present-but-old node
    // (e.g. v14) is the case that used to show a green ✓ while the servers below
    // all say "needs Node LTS" — that's the check lying. Surface it.
    let node_adequate = node
        .as_ref()
        .map(|p| p.version.as_str())
        .and_then(node_major)
        .map(|m| m >= NODE_MIN_MAJOR)
        .unwrap_or(false);
    Json(json!({
        "node": {
            "present": node.is_some(),
            "version": node.as_ref().map(|p| p.version.as_str()),
            "path": node.as_ref().and_then(|p| p.path.as_deref()),
            "adequate": node_adequate,
            "minMajor": NODE_MIN_MAJOR,
        },
        "npm":  {
            "present": npm.is_some(),
            "version": npm.as_ref().map(|p| p.version.as_str()),
            "path": npm.as_ref().and_then(|p| p.path.as_deref()),
        },
        "uv":   {
            "present": uv.is_some(),
            "version": uv.as_ref().map(|p| p.version.as_str()),
            "path": uv.as_ref().and_then(|p| p.path.as_deref()),
        },
    }))
}

// ── 可装的 server allowlist(服务端,防任意命令执行) ─────────────────
// 加新 server = 在 `known` 加一条。命令出处见各自官方 README/docs。

/// 一个可装 server 的元信息。
struct Known {
    command: &'static str,
    args: &'static [&'static str],
    /// 需要 API key 时，key 以这个**环境变量名**传给子进程(如
    /// "CONTEXT7_API_KEY")；None = 无需 key。
    ///
    /// 安全:key 通过 `claude mcp add -e KEY=val …` / `codex mcp add --env
    /// KEY=val …` 传入,**持久落盘时**存在 `mcpServers.<name>.env` /
    /// `[mcp_servers.<name>.env]`,不写进 `args`。注意 `-e/--env` 的值在
    /// `mcp add` 子进程存活的亚秒窗口内仍出现在其 argv,同机同 UID 进程
    /// `ps -ef` 可窥;但能 ps 到它的进程同样能直接读已落盘的同一把 key,
    /// 且本服务是 loopback 单用户桌面场景,故窗口暴露不额外提权。
    api_key_env: Option<&'static str>,
}

fn known(name: &str) -> Option<Known> {
    match name {
        // chrome-devtools README: `npx chrome-devtools-mcp@latest`，无 key。
        "chrome-devtools" => Some(Known {
            command: "npx",
            args: &["-y", "chrome-devtools-mcp@latest"],
            api_key_env: None,
        }),
        // context7 docs:支持用 CONTEXT7_API_KEY 环境变量传 key(官方 all-clients
        // 示例里给的就是 `"env": {"CONTEXT7_API_KEY": "…"}`),优先于 --api-key。
        "context7" => Some(Known {
            command: "npx",
            args: &["-y", "@upstash/context7-mcp"],
            api_key_env: Some("CONTEXT7_API_KEY"),
        }),
        _ => None,
    }
}

fn valid_name(name: &str) -> bool {
    // 安全：name 会被原样塞进 `claude/codex mcp add/remove <name> …` 的参数位。
    // 若允许以 '-' 开头,name 会被 CLI 当成 flag 解析(参数注入),例如名字
    // `--help` / `-s` 之类能改变命令语义。所以:非空、≤64、仅 [A-Za-z0-9_-]、
    // 且**首字符必须是字母或数字**(不得以 '-' 或 '_' 开头)。
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    name.len() <= 64
        && first.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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
// 的配置里把 key 取出来复用 —— 用户只需填一次。
//
// key 现在通过环境变量传/存(见 `Known.api_key_env`),所以读取点是:
//   claude:  mcpServers.<name>.env.<ENV>
//   codex:   [mcp_servers.<name>.env] 段里的 <ENV> = "…"
// 仍兼容历史上以 `--api-key <key>` 写进 args 的旧条目(回退解析),这样老用户
// 升级后第一次还能把旧 key 复用过来。

/// 兼容旧格式:从 args token 里抠出 `--api-key` 后的值。
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

/// claude ~/.claude.json → mcpServers[name].env.<ENV>(优先),回退旧 args 格式。
fn claude_key(name: &str) -> Option<String> {
    let env_name = known(name).and_then(|k| k.api_key_env)?;
    let h = home()?;
    let txt = std::fs::read_to_string(h.join(".claude.json")).ok()?;
    let v: Value = serde_json::from_str(&txt).ok()?;
    let entry = v.get("mcpServers").and_then(|m| m.get(name))?;
    // 首选:env 段里的环境变量值。
    if let Some(key) = entry
        .get("env")
        .and_then(|e| e.get(env_name))
        .and_then(|k| k.as_str())
        .filter(|s| !s.is_empty())
    {
        return Some(key.to_string());
    }
    // 回退:历史上写进 args 的 `--api-key <key>`。
    let args = entry.get("args").and_then(|a| a.as_array())?;
    let toks: Vec<String> = args
        .iter()
        .filter_map(|a| a.as_str().map(String::from))
        .collect();
    extract_key_after_flag(&toks)
}

/// codex ~/.codex/config.toml → [mcp_servers.<name>.env] 段的 <ENV>(优先),
/// 回退旧的 args 行内 `--api-key` 格式。
fn codex_key(name: &str) -> Option<String> {
    let env_name = known(name).and_then(|k| k.api_key_env)?;
    let h = home()?;
    let txt = std::fs::read_to_string(h.join(".codex").join("config.toml")).ok()?;
    let main_header = format!("[mcp_servers.{name}]");
    let env_header = format!("[mcp_servers.{name}.env]");
    let mut in_main = false;
    let mut in_env = false;
    let mut legacy: Option<String> = None;
    for line in txt.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_main = l == main_header;
            in_env = l == env_header;
            continue;
        }
        // 首选:env 段里 `<ENV> = "…"`。
        if in_env {
            if let Some(rest) = l.strip_prefix(env_name) {
                let rest = rest.trim_start();
                if let Some(val) = rest.strip_prefix('=') {
                    let key = val.trim().trim_matches('"');
                    if !key.is_empty() {
                        return Some(key.to_string());
                    }
                }
            }
        }
        // 回退:旧的 args 行里 `"--api-key"` 后(i+2)的引号段。
        if in_main && legacy.is_none() && l.contains("--api-key") {
            let toks: Vec<String> = l.split('"').map(String::from).collect();
            for (i, tk) in toks.iter().enumerate() {
                if tk == "--api-key" {
                    if let Some(v) = toks.get(i + 2) {
                        if !v.is_empty() {
                            legacy = Some(v.clone());
                        }
                    }
                }
            }
        }
    }
    legacy
}

/// 任一 CLI 已配的 key —— 装到另一个 CLI 时复用(claude/codex 共用一把 key)。
fn recover_api_key(name: &str) -> Option<String> {
    claude_key(name).or_else(|| codex_key(name))
}

/// 末 4 位以外打码，用于回显「已设置」状态而不泄露完整 key。
fn mask_key(k: &str) -> String {
    // 按**字符**取尾 4 个,不能用字节切片:多字节 UTF-8 key 的 `k.len()-4`
    // 可能落在某个字符的中间字节上,`&k[..]` 会 panic → /api/mcp/status 500。
    let n = k.chars().count();
    if n <= 4 {
        "••••".to_string()
    } else {
        let tail: String = k.chars().skip(n - 4).collect();
        format!("••••{tail}")
    }
}

// ── 装 / 卸：调 CLI 自己的 `mcp` 子命令 ──────────────────────────────

/// 串行化所有 MCP admin 写操作(install/uninstall)。
///
/// install 是「先 `mcp remove`(忽略不存在)再 `mcp add`」两步,中间不持锁会
/// 与并发的 install/uninstall 撞车:两个并发 install 可能交错成 remove-A →
/// remove-B → add-A → add-B,或一边 add、另一边把它 remove 掉,落到底层
/// `~/.claude.json` / `~/.codex/config.toml` 上还会有读改写竞争。loopback
/// 单用户场景并发概率低,但代价只是一把进程内锁,直接串行化最稳。
static ADMIN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    let (program, resolved_path) = resolved_program(bin);
    let out = crate::runtime_path::tool_command_async(&program)
        .args(args)
        .output()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                "{bin} 启动失败: {e}（已搜索 PATH、Homebrew、用户工具目录；请确认已安装并登录）"
            ),
            )
        })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let label = resolved_path.as_deref().unwrap_or(bin);
        Err((
            StatusCode::BAD_GATEWAY,
            if err.is_empty() {
                format!("{label} 退出码非零")
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
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "非法 server 名"})),
        )
            .into_response();
    }
    let Some(k) = known(&req.name) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("未知 server: {}", req.name)})),
        )
            .into_response();
    };
    let svc_command = crate::runtime_path::resolve_executable(k.command)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| k.command.to_string());
    // server 命令进 `--` 之后的 args(原样,不含 key)。
    let svc_args: Vec<String> = k.args.iter().map(|s| s.to_string()).collect();
    // 需要 key 的 server:解析出 key,用**环境变量**传(不进 argv,见
    // `Known.api_key_env` 的安全说明)。env 段是 KEY=VALUE 串,放在 `--` 之前。
    let mut env_pair: Option<String> = None;
    if let Some(env_name) = k.api_key_env {
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
        env_pair = Some(format!("{env_name}={key}"));
    }
    // claude: `mcp add <name> --scope user [-e KEY=val] -- <cmd> <args...>`
    // codex:  `mcp add <name> [--env KEY=val] -- <cmd> <args...>`
    // 注意:env flag 必须在 `--` 之前(CLI 选项在位置参数之前)。
    let (bin, mut a): (&str, Vec<String>) = match req.cli.as_str() {
        "claude" => {
            let mut v = vec![
                "mcp".into(),
                "add".into(),
                req.name.clone(),
                "--scope".into(),
                "user".into(),
            ];
            if let Some(pair) = &env_pair {
                v.push("-e".into());
                v.push(pair.clone());
            }
            v.push("--".into());
            v.push(svc_command.clone());
            ("claude", v)
        }
        "codex" => {
            let mut v = vec!["mcp".into(), "add".into(), req.name.clone()];
            if let Some(pair) = &env_pair {
                v.push("--env".into());
                v.push(pair.clone());
            }
            v.push("--".into());
            v.push(svc_command);
            ("codex", v)
        }
        _ => return (StatusCode::BAD_REQUEST, Json(json!({"error": "未知 CLI"}))).into_response(),
    };
    a.extend(svc_args);
    // 串行化整段 upsert(remove + add),避免与并发 admin 写竞争。
    let _guard = ADMIN_LOCK.lock().await;
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
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "非法 server 名"})),
        )
            .into_response();
    }
    // 安全:卸载也只限**受管 allowlist** 内的 server。否则这个无 auth 的 loopback
    // 接口能删用户在 ~/.claude.json / ~/.codex 里**任意手配**的 user-scope MCP
    // (例如 sequential-thinking),等于给了任意删除别人配置的能力。这个页面只
    // 负责自己装的那几个,卸载范围必须对称地收回到同一份 known() 清单。
    if known(&req.name).is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("不受管 server，拒绝卸载: {}", req.name)})),
        )
            .into_response();
    }
    // 与 install 串行化(同一把锁),避免并发 add/remove 交错。
    let _guard = ADMIN_LOCK.lock().await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_major_parses_and_gates_lts() {
        assert_eq!(node_major("v22.17.0"), Some(22));
        assert_eq!(node_major("v18.20.8"), Some(18));
        assert_eq!(node_major("14.21.3"), Some(14)); // tolerate a missing 'v'
        assert_eq!(node_major("not-a-version"), None);
        // The gate the UI relies on: v14 is below LTS, v18/v22 are not.
        assert!(node_major("v14.21.3").unwrap() < NODE_MIN_MAJOR);
        assert!(node_major("v18.20.8").unwrap() >= NODE_MIN_MAJOR);
    }

    #[test]
    fn valid_name_rejects_flag_like_and_bad_chars() {
        // 正常名字放行。
        assert!(valid_name("chrome-devtools"));
        assert!(valid_name("context7"));
        assert!(valid_name("a"));
        assert!(valid_name("a_b-c9"));
        // 安全核心:不得以 '-' 开头(否则被 CLI 当 flag → 参数注入)。
        assert!(!valid_name("-rf"));
        assert!(!valid_name("--help"));
        assert!(!valid_name("-s"));
        // 也不得以 '_' 开头(首字符必须字母/数字)。
        assert!(!valid_name("_x"));
        // 空 / 超长 / 非法字符。
        assert!(!valid_name(""));
        assert!(!valid_name(&"a".repeat(65)));
        assert!(!valid_name("a b"));
        assert!(!valid_name("a;b"));
        assert!(!valid_name("a/b"));
    }

    #[test]
    fn mask_key_is_utf8_safe_and_hides_body() {
        // 短 key 全打码。
        assert_eq!(mask_key(""), "••••");
        assert_eq!(mask_key("abcd"), "••••");
        // ASCII:露末 4。
        assert_eq!(mask_key("abcdef"), "••••cdef");
        // 多字节 UTF-8:旧的字节切片在 `len()-4` 落到字符中间会 panic;
        // 这里只要不 panic 且尾 4 个**字符**正确即可。"密钥🔑值末四" 末 4 字符是 "值末四"… 取按 char。
        let k = "🔑🔑🔑🔑🔑🔑"; // 6 个 4-byte 字符
        assert_eq!(mask_key(k), "••••🔑🔑🔑🔑");
        // 混合多字节,确认不 panic。
        let _ = mask_key(" key=值密钥末尾");
    }

    #[test]
    fn uninstall_gate_uses_known_allowlist() {
        // 卸载的 allowlist 与安装同源:受管的放行,任意手配的拒绝。
        assert!(known("chrome-devtools").is_some());
        assert!(known("context7").is_some());
        assert!(known("sequential-thinking").is_none());
        assert!(known("anything-else").is_none());
    }
}
