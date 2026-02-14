use rust_decimal::Decimal;

/// Tracks position limits, exposure caps, and drawdown.
#[derive(Debug)]
#[allow(dead_code)]
pub struct PositionLimits {
    max_exposure: Decimal,
    current_exposure: Decimal,
    daily_loss: Decimal,
    consecutive_failures: u32,
}

impl PositionLimits {
    pub fn new(max_exposure: Decimal) -> Self {
        Self {
            max_exposure,
            current_exposure: Decimal::ZERO,
            daily_loss: Decimal::ZERO,
            consecutive_failures: 0,
        }
    }

    pub fn current_exposure(&self) -> Decimal {
        self.current_exposure
    }

    #[allow(dead_code)]
    pub fn daily_loss(&self) -> Decimal {
        self.daily_loss
    }

    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    pub fn can_add_exposure(&self, amount: Decimal) -> bool {
        self.current_exposure + amount <= self.max_exposure
    }

    pub fn add_exposure(&mut self, amount: Decimal) {
        self.current_exposure += amount;
    }

    #[allow(dead_code)]
    pub fn add_daily_loss(&mut self, amount: Decimal) {
        self.daily_loss += amount;
    }

    pub fn increment_failures(&mut self) {
        self.consecutive_failures += 1;
    }

    pub fn reset_failures(&mut self) {
        self.consecutive_failures = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn exposure_limits() {
        let mut limits = PositionLimits::new(dec!(1000));
        assert!(limits.can_add_exposure(dec!(500)));
        limits.add_exposure(dec!(500));
        assert!(limits.can_add_exposure(dec!(500)));
        assert!(!limits.can_add_exposure(dec!(501)));
    }

    #[test]
    fn failure_tracking() {
        let mut limits = PositionLimits::new(dec!(1000));
        assert_eq!(limits.consecutive_failures(), 0);
        limits.increment_failures();
        limits.increment_failures();
        assert_eq!(limits.consecutive_failures(), 2);
        limits.reset_failures();
        assert_eq!(limits.consecutive_failures(), 0);
    }

    #[test]
    fn daily_loss_tracking() {
        let mut limits = PositionLimits::new(dec!(1000));
        limits.add_daily_loss(dec!(10));
        limits.add_daily_loss(dec!(5));
        assert_eq!(limits.daily_loss(), dec!(15));
    }
}
