use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;

use rust_decimal::Decimal;
use serde::Serialize;
use tracing::{error, info};

use crate::arb::types::ArbitrageOpportunity;

/// A single journal entry written to JSONL.
#[derive(Debug, Serialize)]
pub struct JournalEntry {
    pub timestamp: String,
    pub detected_at_ms: u64,
    pub path: String,
    pub start_asset: String,
    pub start_amount: Decimal,
    pub end_amount: Decimal,
    pub profit_amount: Decimal,
    pub profit_pct: Decimal,
    pub dry_run: bool,
    pub outcome: TradeOutcome,
    pub legs: Vec<JournalLeg>,
}

#[derive(Debug, Serialize)]
pub struct JournalLeg {
    pub symbol: String,
    pub side: String,
    pub quantity: Decimal,
    pub effective_price: Decimal,
    pub input_amount: Decimal,
    pub output_amount: Decimal,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TradeOutcome {
    Executed,
    DryRun,
    Failed { leg: usize, reason: String },
}

pub struct TradeJournal {
    writer: Mutex<BufWriter<File>>,
}

impl TradeJournal {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        info!("trade journal: {}", path.display());
        Ok(Self {
            writer: Mutex::new(BufWriter::new(file)),
        })
    }

    pub fn record(
        &self,
        opp: &ArbitrageOpportunity,
        outcome: TradeOutcome,
        dry_run: bool,
    ) {
        let entry = JournalEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            detected_at_ms: opp.detected_at_ms,
            path: opp.path.to_string(),
            start_asset: opp.path.start_asset.clone(),
            start_amount: opp.start_amount,
            end_amount: opp.end_amount,
            profit_amount: opp.profit_amount,
            profit_pct: opp.profit_pct,
            dry_run,
            outcome,
            legs: opp
                .legs
                .iter()
                .map(|l| JournalLeg {
                    symbol: l.symbol.clone(),
                    side: l.side.to_string(),
                    quantity: l.quantity,
                    effective_price: l.effective_price,
                    input_amount: l.input_amount,
                    output_amount: l.output_amount,
                })
                .collect(),
        };

        let mut w = self.writer.lock().expect("journal lock poisoned");
        match serde_json::to_string(&entry) {
            Ok(json) => {
                if let Err(e) = writeln!(w, "{}", json) {
                    error!("failed to write journal: {}", e);
                }
                let _ = w.flush();
            }
            Err(e) => {
                error!("failed to serialize journal entry: {}", e);
            }
        }
    }
}
