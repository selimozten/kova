use anyhow::{Context, Result};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub general: GeneralConfig,
    pub exchange: ExchangeConfig,
    pub triangles: TrianglesConfig,
    pub risk: RiskConfig,
    #[serde(default)]
    pub monitoring: MonitoringConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GeneralConfig {
    pub start_asset: String,
    pub trade_amount: Decimal,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_log_level() -> String {
    "info".into()
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExchangeConfig {
    pub name: String,
    pub api_key: String,
    pub api_secret: String,
    pub base_url: Option<String>,
    pub ws_url: Option<String>,
    #[serde(default)]
    pub fees: FeeConfig,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct FeeConfig {
    #[serde(default = "default_fee")]
    pub maker: Decimal,
    #[serde(default = "default_fee")]
    pub taker: Decimal,
}

fn default_fee() -> Decimal {
    Decimal::new(1, 3) // 0.001
}

#[derive(Debug, Deserialize, Clone)]
pub struct TrianglesConfig {
    #[serde(default = "default_true")]
    pub auto_discover: bool,
    #[serde(default)]
    pub start_assets: Vec<String>,
    #[serde(default)]
    pub paths: Vec<Vec<String>>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Clone)]
pub struct RiskConfig {
    #[serde(default = "default_min_profit")]
    pub min_profit_pct: Decimal,
    #[serde(default = "default_max_trade_size")]
    pub max_trade_size: Decimal,
    #[serde(default = "default_max_session_loss")]
    pub max_session_loss: Decimal,
    #[serde(default = "default_max_consecutive_losses")]
    pub max_consecutive_losses: u32,
    #[serde(default = "default_slippage_buffer")]
    pub slippage_buffer_bps: Decimal,
    #[serde(default = "default_cooldown")]
    pub cooldown_ms: u64,
}

fn default_min_profit() -> Decimal {
    Decimal::new(15, 2) // 0.15
}
fn default_max_trade_size() -> Decimal {
    Decimal::new(500, 0)
}
fn default_max_session_loss() -> Decimal {
    Decimal::new(50, 0)
}
fn default_max_consecutive_losses() -> u32 {
    5
}
fn default_slippage_buffer() -> Decimal {
    Decimal::new(2, 0)
}
fn default_cooldown() -> u64 {
    500
}

#[derive(Debug, Deserialize, Clone)]
pub struct MonitoringConfig {
    #[serde(default = "default_true")]
    pub print_opportunities: bool,
    #[serde(default = "default_stats_interval")]
    pub stats_interval_secs: u64,
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            print_opportunities: true,
            stats_interval_secs: default_stats_interval(),
        }
    }
}

fn default_stats_interval() -> u64 {
    10
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading config: {}", path.display()))?;
        let config: Config =
            toml::from_str(&content).with_context(|| format!("parsing config: {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(!self.general.start_asset.is_empty(), "start_asset must not be empty");
        anyhow::ensure!(
            self.general.trade_amount > Decimal::ZERO,
            "trade_amount must be positive"
        );
        anyhow::ensure!(!self.exchange.name.is_empty(), "exchange name must not be empty");
        anyhow::ensure!(!self.exchange.api_key.is_empty(), "api_key must not be empty");
        anyhow::ensure!(!self.exchange.api_secret.is_empty(), "api_secret must not be empty");
        anyhow::ensure!(
            self.risk.min_profit_pct >= Decimal::ZERO,
            "min_profit_pct must be non-negative"
        );
        anyhow::ensure!(
            self.risk.max_trade_size > Decimal::ZERO,
            "max_trade_size must be positive"
        );

        if !self.triangles.auto_discover && self.triangles.paths.is_empty() {
            anyhow::bail!("either auto_discover must be true or explicit paths must be provided");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let toml_str = r#"
[general]
start_asset = "USDT"
trade_amount = 100.0
dry_run = true

[exchange]
name = "binance"
api_key = "test_key"
api_secret = "test_secret"

[exchange.fees]
maker = 0.001
taker = 0.001

[triangles]
auto_discover = true
start_assets = ["USDT"]

[risk]
min_profit_pct = 0.15
max_trade_size = 500.0
max_session_loss = 50.0
max_consecutive_losses = 5
slippage_buffer_bps = 2.0
cooldown_ms = 500

[monitoring]
print_opportunities = true
stats_interval_secs = 10
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.general.start_asset, "USDT");
        assert_eq!(config.exchange.name, "binance");
        assert!(config.general.dry_run);
    }

    #[test]
    fn test_invalid_config() {
        let toml_str = r#"
[general]
start_asset = ""
trade_amount = 100.0

[exchange]
name = "binance"
api_key = "test"
api_secret = "test"

[triangles]
auto_discover = true

[risk]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.validate().is_err());
    }
}
