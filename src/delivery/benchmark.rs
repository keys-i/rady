use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Result;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Measurement {
    pub command: String,
    pub metric: String,
    pub samples: Vec<f64>,
    pub median: f64,
    pub relative_mad_percent: f64,
    pub warmups: usize,
    pub runs: usize,
}

impl Measurement {
    fn validate(&self) -> Result<()> {
        if self.command.is_empty()
            || self.metric.is_empty()
            || !(3..=30).contains(&self.runs)
            || self.warmups > 5
            || self.samples.len() != self.runs
            || self.samples.iter().any(|value| !positive(*value))
        {
            bail!("benchmark measurement is incomplete");
        }
        let median = median(&self.samples)?;
        let spread = relative_mad(&self.samples, median)?;
        if !approximately(self.median, median) || !approximately(self.relative_mad_percent, spread)
        {
            bail!("benchmark summary does not match its samples");
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub fn measure<F>(
    commands: &[Vec<String>],
    directory: &Path,
    timeout: Duration,
    scratch: &Path,
    label: &str,
    runs: usize,
    warmups: usize,
    metric: Option<&str>,
    mut check: F,
) -> Result<Vec<Measurement>>
where
    F: FnMut(&[String], &Path, Duration) -> Result<(i32, String)>,
{
    if commands.is_empty() || commands.iter().any(Vec::is_empty) {
        bail!("provide at least one nonempty benchmark command");
    }
    if !(3..=30).contains(&runs) || warmups > 5 || timeout.is_zero() {
        bail!("use 3-30 benchmark runs, 0-5 warmups and a positive timeout");
    }
    if label.is_empty()
        || !label
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        bail!("benchmark label may contain only letters, numbers, underscores and hyphens");
    }
    if metric.is_some_and(|value| value.trim().is_empty()) {
        bail!("benchmark metric must be a nonempty JSON field name");
    }
    fs::create_dir_all(scratch)?;
    let mut results = Vec::with_capacity(commands.len());
    for (index, command) in commands.iter().enumerate() {
        let command_name = join_command(command);
        for repeat in 1..=warmups {
            let (status, output) = check(command, directory, timeout)?;
            fs::write(
                scratch.join(format!(
                    "benchmark-{label}-{}-warmup-{repeat}.log",
                    index + 1
                )),
                &output,
            )?;
            if status != 0 {
                bail!("benchmark failed during warmup: {command_name}");
            }
        }
        let mut samples = Vec::with_capacity(runs);
        for repeat in 1..=runs {
            let started = Instant::now();
            let (status, output) = check(command, directory, timeout)?;
            let elapsed = started.elapsed().as_secs_f64();
            fs::write(
                scratch.join(format!(
                    "benchmark-{label}-{}-sample-{repeat}.log",
                    index + 1
                )),
                &output,
            )?;
            if status != 0 {
                bail!("benchmark failed: {command_name}");
            }
            samples.push(match metric {
                Some(metric) => output_metric(&output, metric)?,
                None if positive(elapsed) => elapsed,
                None => f64::EPSILON,
            });
        }
        let middle = median(&samples)?;
        results.push(Measurement {
            command: command_name,
            metric: metric.unwrap_or("wall_seconds").to_owned(),
            relative_mad_percent: relative_mad(&samples, middle)?,
            samples,
            median: middle,
            warmups,
            runs,
        });
    }
    Ok(results)
}

pub fn compare(
    before: &[Measurement],
    after: &[Measurement],
    max_regression: f64,
    max_noise: f64,
) -> Result<Vec<String>> {
    if !nonnegative(max_regression) || !nonnegative(max_noise) {
        bail!("benchmark limits must be finite non-negative numbers");
    }
    if before.len() != after.len() {
        return Ok(vec![
            "Benchmark command count changed between baseline and candidate".to_owned(),
        ]);
    }
    let mut blockers = Vec::new();
    for (index, (baseline, candidate)) in before.iter().zip(after).enumerate() {
        let baseline_valid = baseline.validate();
        let candidate_valid = candidate.validate();
        if let Err(error) = &baseline_valid {
            blockers.push(format!("Baseline benchmark {} {error}", index + 1));
        }
        if let Err(error) = &candidate_valid {
            blockers.push(format!("Candidate benchmark {} {error}", index + 1));
        }
        if baseline_valid.is_err() || candidate_valid.is_err() {
            continue;
        }
        if baseline.command != candidate.command || baseline.metric != candidate.metric {
            blockers.push(format!(
                "Benchmark command or metric changed at position {}",
                index + 1
            ));
            continue;
        }
        if baseline.samples.len() != candidate.samples.len() {
            blockers.push(format!(
                "Benchmark sample count changed for {}",
                baseline.command
            ));
            continue;
        }
        if baseline.relative_mad_percent > max_noise || candidate.relative_mad_percent > max_noise {
            blockers.push(format!(
                "Benchmark noise exceeds {max_noise}% for {}; trusted comparison is blocked",
                baseline.command
            ));
            continue;
        }
        let regression = (candidate.median - baseline.median) / baseline.median * 100.0;
        if regression > max_regression {
            blockers.push(format!(
                "Benchmark regressed {regression:.1}% for {} (limit {max_regression}%)",
                baseline.command
            ));
        }
    }
    Ok(blockers)
}

fn output_metric(output: &str, metric: &str) -> Result<f64> {
    let value: Value = serde_json::from_str(output)
        .map_err(|_| anyhow!("benchmark did not emit a JSON object for metric {metric}"))?;
    let number = value
        .as_object()
        .and_then(|object| object.get(metric))
        .and_then(Value::as_f64)
        .filter(|value| positive(*value))
        .ok_or_else(|| anyhow!("benchmark JSON must contain positive numeric metric {metric}"))?;
    Ok(number)
}

fn median(values: &[f64]) -> Result<f64> {
    if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
        bail!("benchmark samples must be finite and nonempty");
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    Ok(if sorted.len() % 2 == 0 {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    })
}

fn relative_mad(samples: &[f64], middle: f64) -> Result<f64> {
    let deviations: Vec<_> = samples
        .iter()
        .map(|sample| (sample - middle).abs())
        .collect();
    Ok(median(&deviations)? / middle * 100.0)
}

fn join_command(command: &[String]) -> String {
    command
        .iter()
        .map(|value| {
            if value
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-._/:".contains(character))
            {
                value.clone()
            } else {
                format!("'{}'", value.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn approximately(left: f64, right: f64) -> bool {
    (left - right).abs() <= f64::EPSILON * left.abs().max(right.abs()).max(1.0) * 8.0
}

fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

fn nonnegative(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measurement(median: f64, noise: f64) -> Measurement {
        Measurement {
            command: "bench".to_owned(),
            metric: "seconds".to_owned(),
            samples: vec![median; 3],
            median,
            relative_mad_percent: noise,
            warmups: 1,
            runs: 3,
        }
    }

    #[test]
    fn table_driven_comparisons_cover_pass_regression_noise_and_shape() -> Result<()> {
        let cases = [
            (
                vec![measurement(10.0, 0.0)],
                vec![measurement(10.4, 0.0)],
                false,
            ),
            (
                vec![measurement(10.0, 0.0)],
                vec![measurement(10.6, 0.0)],
                true,
            ),
            (
                vec![measurement(10.0, 11.0)],
                vec![measurement(10.0, 0.0)],
                true,
            ),
            (vec![measurement(10.0, 0.0)], vec![], true),
        ];
        for (before, after, blocked) in cases {
            assert_eq!(!compare(&before, &after, 5.0, 10.0)?.is_empty(), blocked);
        }
        Ok(())
    }

    #[test]
    fn table_driven_metric_parser_rejects_invalid_values() {
        for (source, valid) in [
            (r#"{"memory":42.5}"#, true),
            (r#"{"memory":0}"#, false),
            (r#"{"other":42}"#, false),
            ("no", false),
        ] {
            assert_eq!(output_metric(source, "memory").is_ok(), valid, "{source}");
        }
    }
}
