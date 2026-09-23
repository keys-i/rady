mod evaluate;
mod harness;
mod process;

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

pub use evaluate::{evaluate, evaluate_cancellable, worker_message};
pub use harness::{AgentCommand, command, executable, run, run_cancellable, split_command, which};
pub use process::{MAX_OUTPUT, ProcessOutput, execute, safe_environment};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Codex,
    Claude,
    Command,
}

impl Harness {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Command => "command",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct UsageRecord {
    pub harness: String,
    pub known: bool,
    #[serde(flatten)]
    pub values: BTreeMap<String, u64>,
}

#[derive(Debug, Default)]
pub struct Usage {
    max_tokens: Option<u64>,
    records: Vec<UsageRecord>,
    missing: bool,
}

impl Usage {
    pub fn new(max_tokens: Option<u64>) -> Result<Self> {
        if max_tokens == Some(0) {
            bail!("token budget must be a positive whole number");
        }
        Ok(Self {
            max_tokens,
            records: Vec::new(),
            missing: false,
        })
    }

    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.records
            .iter()
            .filter_map(|record| record.values.get("total_tokens"))
            .sum()
    }

    pub fn before_call(&self) -> Result<()> {
        let Some(maximum) = self.max_tokens else {
            return Ok(());
        };
        if self.missing {
            bail!("token usage is unavailable, so Rady cannot enforce the budget");
        }
        if self.total_tokens() >= maximum {
            bail!("token budget is exhausted");
        }
        Ok(())
    }

    pub fn record(
        &mut self,
        harness: Harness,
        values: Option<BTreeMap<String, u64>>,
    ) -> Result<()> {
        let known = values.is_some();
        if !known {
            self.missing = true;
        }
        self.records.push(UsageRecord {
            harness: harness.as_str().to_owned(),
            known,
            values: values.unwrap_or_default(),
        });
        if !known && self.max_tokens.is_some() {
            bail!("token usage is unavailable, so Rady cannot enforce the budget");
        }
        if self
            .max_tokens
            .is_some_and(|maximum| self.total_tokens() > maximum)
        {
            bail!("token budget was exceeded");
        }
        Ok(())
    }

    pub fn record_unknown(&mut self, harness: Harness) {
        let _ = self.record(harness, None);
    }

    #[must_use]
    pub fn complete(&self, agents: usize) -> bool {
        !self.missing && agents == 1
    }

    #[must_use]
    pub fn records(&self) -> &[UsageRecord] {
        &self.records
    }

    #[must_use]
    pub const fn maximum(&self) -> Option<u64> {
        self.max_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_budget_fails_closed() -> Result<()> {
        let mut usage = Usage::new(Some(4))?;
        usage
            .record(
                Harness::Command,
                Some(BTreeMap::from([("total_tokens".to_owned(), 5)])),
            )
            .expect_err("budget must reject overshoot");
        assert_eq!(usage.total_tokens(), 5);
        Ok(())
    }
}
