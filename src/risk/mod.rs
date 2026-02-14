pub mod kill_switch;
pub mod limits;

use std::sync::RwLock;

use rust_decimal::Decimal;
use tracing::{info, warn};

use crate::config::types::RiskConfig;
use crate::error::{Error, Result};
use crate::strategy::Opportunity;

use kill_switch::KillSwitch;
use limits::PositionLimits;

/// Central risk manager that gates every execution.
/// All state is behind RwLock for fast, synchronous access.
#[allow(dead_code)]
pub struct RiskManager {
    config: RiskConfig,
    state: RwLock<RiskState>,
}

struct RiskState {
    kill_switch: KillSwitch,
    limits: PositionLimits,
}

impl RiskManager {
    pub fn new(config: RiskConfig) -> Self {
        let state = RiskState {
            kill_switch: KillSwitch::new(),
            limits: PositionLimits::new(config.max_exposure_usdc),
        };
        Self {
            config,
            state: RwLock::new(state),
        }
    }

    /// Check if an opportunity passes all risk checks.
    /// Returns Ok(()) if allowed, Err with reason if blocked.
    pub fn check(&self, opportunity: &Opportunity) -> Result<()> {
        let state = self
            .state
            .read()
            .map_err(|_| Error::Risk("lock poisoned".into()))?;

        // Kill switch check
        if state.kill_switch.is_killed() {
            return Err(Error::Risk(format!(
                "kill switch active: {}",
                state.kill_switch.reason()
            )));
        }

        // Exposure limit check
        if !state
            .limits
            .can_add_exposure(opportunity.suggested_size_usdc)
        {
            return Err(Error::Risk(format!(
                "exposure limit exceeded: current={}, requested={}, max={}",
                state.limits.current_exposure(),
                opportunity.suggested_size_usdc,
                self.config.max_exposure_usdc,
            )));
        }

        Ok(())
    }

    /// Record a realized loss. Triggers kill switch if thresholds breached.
    #[allow(dead_code)]
    pub fn record_loss(&self, amount: Decimal) {
        if let Ok(mut state) = self.state.write() {
            state.limits.add_daily_loss(amount);
            if state.limits.daily_loss() >= self.config.max_daily_loss_usdc {
                let reason = format!(
                    "daily loss limit reached: {} >= {}",
                    state.limits.daily_loss(),
                    self.config.max_daily_loss_usdc
                );
                warn!("{}", reason);
                state.kill_switch.trigger(&reason);
            }
        }
    }

    /// Record a successful execution (adds to exposure).
    pub fn record_execution(&self, size_usdc: Decimal) {
        if let Ok(mut state) = self.state.write() {
            state.limits.add_exposure(size_usdc);
        }
    }

    /// Record a consecutive failure. Triggers kill switch if threshold breached.
    pub fn record_failure(&self) {
        if let Ok(mut state) = self.state.write() {
            state.limits.increment_failures();
            if state.limits.consecutive_failures() >= self.config.max_consecutive_failures {
                let reason = format!(
                    "too many consecutive failures: {}",
                    state.limits.consecutive_failures()
                );
                warn!("{}", reason);
                state.kill_switch.trigger(&reason);
            }
        }
    }

    /// Reset consecutive failure counter (on success).
    pub fn reset_failures(&self) {
        if let Ok(mut state) = self.state.write() {
            state.limits.reset_failures();
        }
    }

    /// Manually trigger the kill switch.
    #[allow(dead_code)]
    pub fn kill(&self, reason: &str) {
        if let Ok(mut state) = self.state.write() {
            state.kill_switch.trigger(reason);
            warn!(reason, "kill switch manually triggered");
        }
    }

    /// Resume trading (reset kill switch).
    #[allow(dead_code)]
    pub fn resume(&self) {
        if let Ok(mut state) = self.state.write() {
            state.kill_switch.reset();
            info!("kill switch reset, trading resumed");
        }
    }

    #[allow(dead_code)]
    pub fn is_killed(&self) -> bool {
        self.state
            .read()
            .map(|s| s.kill_switch.is_killed())
            .unwrap_or(true)
    }
}
