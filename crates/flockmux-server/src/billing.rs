//! Provider billing guardrails.
//!
//! These helpers centralize the "do not silently move Claude off the
//! interactive subscription PTY" rule. Routes and spawn paths should ask this
//! module instead of each carrying their own env-var names and warning copy.

use crate::plugins::{BillingSurface, CliPlugin};

pub const PAID_TRANSPORT_ENV: &str = "FLOCKMUX_ALLOW_PAID_TRANSPORT";
pub const CLAUDE_PRINT_ENV: &str = "FLOCKMUX_ALLOW_CLAUDE_PRINT";

pub fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

pub fn paid_transport_opt_in_enabled() -> bool {
    env_truthy(PAID_TRANSPORT_ENV)
}

pub fn claude_print_opt_in_enabled() -> bool {
    env_truthy(CLAUDE_PRINT_ENV)
}

pub fn enforce_spawn_billing_policy(plugin: &CliPlugin) -> Result<(), String> {
    if plugin.requires_explicit_billing_opt_in
        && !paid_transport_opt_in_enabled()
        && matches!(
            plugin.billing_surface,
            BillingSurface::AgentSdkCredits | BillingSurface::ApiKey
        )
    {
        return Err(format!(
            "CLI plugin `{}` uses billing surface `{:?}` and requires explicit opt-in. \
             Set {PAID_TRANSPORT_ENV}=1 only if you intend to use SDK/API billing.",
            plugin.id, plugin.billing_surface
        ));
    }

    Ok(())
}

pub fn claude_print_block_message(action: &str) -> String {
    format!(
        "{action}默认禁用：它会调用 `claude -p`，该路径可能消耗 Claude Agent SDK/API 额度而不是交互式订阅额度。\
         若你明确接受这个计费面，设置 {CLAUDE_PRINT_ENV}=1 后重启 flockmux。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_truthy_accepts_explicit_opt_in_values() {
        std::env::set_var("__FLOCKMUX_TEST_TRUTHY", "yes");
        assert!(env_truthy("__FLOCKMUX_TEST_TRUTHY"));
        std::env::set_var("__FLOCKMUX_TEST_TRUTHY", "0");
        assert!(!env_truthy("__FLOCKMUX_TEST_TRUTHY"));
        std::env::remove_var("__FLOCKMUX_TEST_TRUTHY");
    }

    #[test]
    fn claude_print_message_names_the_single_gate() {
        let msg = claude_print_block_message("提示词优化");
        assert!(msg.contains("claude -p"));
        assert!(msg.contains(CLAUDE_PRINT_ENV));
    }
}
