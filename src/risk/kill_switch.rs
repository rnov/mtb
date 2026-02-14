/// Global kill switch that halts all trading.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct KillSwitch {
    killed: bool,
    reason: String,
}

impl KillSwitch {
    pub fn new() -> Self {
        Self {
            killed: false,
            reason: String::new(),
        }
    }

    pub fn is_killed(&self) -> bool {
        self.killed
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn trigger(&mut self, reason: &str) {
        self.killed = true;
        self.reason = reason.to_string();
    }

    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.killed = false;
        self.reason.clear();
    }
}
