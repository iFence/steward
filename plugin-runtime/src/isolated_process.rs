//! Dedicated-process isolation (`--dedicated`).
//!
//! The same binary that serves the shared pool can run in dedicated mode:
//! exactly one plugin per process. `plugin-host` spawns one instance per
//! plugin that declares `isolation: dedicated-process` (privileged or heavy
//! plugins), so a crash, heap blowout or infinite loop can at worst kill that
//! plugin's own process. The service loop and pool remain identical; only the
//! configuration differs (single slot, replace-on-load).

use crate::isolate_pool::{DEFAULT_HEAP_LIMIT, DEFAULT_MAX_STACK};
use crate::service::ServiceConfig;

/// Service configuration for a dedicated runtime process.
pub fn dedicated_config() -> ServiceConfig {
    ServiceConfig {
        dedicated: true,
        pool_capacity: 1,
        heap_limit: DEFAULT_HEAP_LIMIT,
        max_stack: DEFAULT_MAX_STACK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedicated_config_hosts_one_isolate() {
        let config = dedicated_config();
        assert!(config.dedicated);
        assert_eq!(config.pool_capacity, 1);
    }
}
