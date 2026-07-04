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
