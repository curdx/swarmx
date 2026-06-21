//! Provider billing guardrails.
//!
//! These helpers centralize the "do not silently move Claude off the
//! interactive subscription PTY" rule. Routes and spawn paths should ask this
//! module instead of each carrying their own env-var names and warning copy.

use crate::plugins::{BillingSurface, CliPlugin};

pub const PAID_TRANSPORT_ENV: &str = "SWARMX_ALLOW_PAID_TRANSPORT";

pub fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

pub fn paid_transport_opt_in_enabled() -> bool {
    env_truthy(PAID_TRANSPORT_ENV)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_truthy_accepts_explicit_opt_in_values() {
        std::env::set_var("__SWARMX_TEST_TRUTHY", "yes");
        assert!(env_truthy("__SWARMX_TEST_TRUTHY"));
        std::env::set_var("__SWARMX_TEST_TRUTHY", "0");
        assert!(!env_truthy("__SWARMX_TEST_TRUTHY"));
        std::env::remove_var("__SWARMX_TEST_TRUTHY");
    }
}
