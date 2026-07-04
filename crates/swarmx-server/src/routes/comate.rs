//! `GET/PUT /api/comate` — the Comate Zulu license the settings page manages.
//! zulu needs an explicit SaaS license (not OAuth like claude, not an env-only
//! key like reasonix); this exposes read (masked) + write over the loopback API.
//! Storage + resolution live in `crate::comate`.

use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::Deserialize;
use serde_json::json;

/// `GET /api/comate` → whether a license is configured, where it came from, and
/// a masked hint (never the full secret — mirrors mcp_status's last-4 masking).
pub async fn get_license() -> impl IntoResponse {
    let (lic, source) = crate::comate::resolve();
    let src = match source {
        crate::comate::Source::Env => "env",
        crate::comate::Source::File => "file",
        crate::comate::Source::None => "none",
    };
    Json(json!({
        "configured": !lic.is_empty(),
        "source": src,
        "hint": if lic.is_empty() { String::new() } else { crate::comate::mask(&lic) },
    }))
}

#[derive(Deserialize)]
pub struct LicenseBody {
    pub license: String,
}

/// `PUT /api/comate {license}` → persist (empty clears). When `COMATE_LICENSE`
/// is set in the environment it wins at spawn regardless; the file value is
/// still saved so removing the env var later restores it.
pub async fn put_license(Json(body): Json<LicenseBody>) -> impl IntoResponse {
    match crate::comate::save_license(&body.license) {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /api/zulu/models` → the models zulu can run under the configured license
/// (`zulu list-model`), as `[{modelId, displayName, thinking, image}]`. Powers
/// the model picker for zulu agents and the fusion panel (one license, N
/// models). Runs the CLI directly (not the shim) — a fast, read-only query.
pub async fn zulu_models() -> impl IntoResponse {
    let license = crate::comate::load_license();
    if license.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "未配置 Comate License（设置 → 插件）" })),
        )
            .into_response();
    }
    let out = tokio::process::Command::new("zulu")
        .args(["list-model", "-l", license.trim()])
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => match serde_json::from_slice::<serde_json::Value>(&o.stdout) {
            Ok(v) => Json(v).into_response(),
            Err(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "无法解析 zulu list-model 输出" })),
            )
                .into_response(),
        },
        Ok(o) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": String::from_utf8_lossy(&o.stderr).trim().to_string() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": format!("zulu 不可运行：{e}（是否已 npm i -g @comate/zulu？）") })),
        )
            .into_response(),
    }
}
