//! Bounded, durable, opt-in transient recovery. Persist intent BEFORE input, so a crash or
//! uncertain outcome never causes an automatic replay. Each episode gets three attempts.
use super::*;
use crate::stuck::StuckCause;

#[derive(Default, Deserialize, Serialize)]
struct Receipt {
    episode: String,
    attempts: u32,
    next_at: f64,
    uncertain: bool,
}

pub(super) fn recover_due(
    report: &Value,
    notifier: &mut dyn Notifier,
    now: f64,
    options: &WatchAgentsOptions,
) -> Result<Value> {
    if options.check_only {
        return Ok(json!([]));
    }
    let mut results = Vec::new();
    for row in report["episodes"].as_array().into_iter().flatten() {
        let Some(agent) = row["agent"].as_str() else {
            continue;
        };
        if !options.auto_recover.iter().any(|name| name == agent) {
            continue;
        }
        let cause = match row["kind"].as_str() {
            Some("model-capacity") => StuckCause::ModelCapacity,
            Some("api-rate-limit") => StuckCause::ApiRateLimit,
            _ => continue,
        };
        let Some((state_path, _)) = state_paths(options) else {
            return Err(fail("auto-recovery requires a durable state directory"));
        };
        crate::subagents::name(agent)?;
        let path = state_path.with_file_name(format!("recovery-{agent}.json"));
        let episode = format!("{}:{}", cause.as_str(), row["since"]);
        let mut receipt: Receipt = if path.try_exists().map_err(|e| fail(e.to_string()))? {
            serde_json::from_value(crate::agent::read_private_json(&path)?)
                .map_err(|e| fail(format!("cannot parse recovery receipt: {e}")))?
        } else {
            Receipt::default()
        };
        if receipt.episode != episode {
            // Uncertainty requires an operator even if a new episode appeared.
            if !receipt.uncertain {
                receipt = Receipt {
                    episode,
                    next_at: now + 60.0,
                    ..Receipt::default()
                };
                write_json_file(
                    &path,
                    &serde_json::to_value(&receipt).map_err(|e| fail(e.to_string()))?,
                )?;
            }
        }
        let outcome = if receipt.uncertain {
            "operator required: previous recovery outcome uncertain".to_owned()
        } else if receipt.attempts >= 3 {
            "operator required: three recovery attempts exhausted".to_owned()
        } else if now < receipt.next_at {
            format!("backoff: retry in {:.0} seconds", receipt.next_at - now)
        } else {
            receipt.attempts += 1;
            receipt.uncertain = true;
            write_json_file(
                &path,
                &serde_json::to_value(&receipt).map_err(|e| fail(e.to_string()))?,
            )?;
            let outcome = match notifier.recover_transient(agent, cause) {
                Ok(detail) => {
                    receipt.uncertain = false;
                    detail
                }
                Err(error) => format!("operator required: recovery failed or uncertain: {error}"),
            };
            receipt.next_at = now + f64::from(60_u32 << receipt.attempts);
            write_json_file(
                &path,
                &serde_json::to_value(&receipt).map_err(|e| fail(e.to_string()))?,
            )?;
            let notice = format!(
                "[agentctl watch-agents] {agent}: recovery attempt {}: {outcome}",
                receipt.attempts
            );
            let notice_id = format!(
                "recovery-{agent}-{:x}-{}",
                stable_hash(&receipt.episode),
                receipt.attempts
            );
            let target = options.coordinator.as_str();
            if let Err(error) = notifier.notify(target, &notice, &notice_id) {
                eprintln!("agentctl watch-agents: recovery notice failed: {error}");
            }
            eprintln!(
                "agentctl watch-agents: {agent} recovery attempt {}: {outcome}",
                receipt.attempts
            );
            outcome
        };
        results.push(json!({"agent": agent, "cause": cause, "attempts": receipt.attempts, "outcome": outcome}));
    }
    Ok(json!(results))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Sink {
        attempts: usize,
        fail: bool,
    }
    impl Notifier for Sink {
        fn notify(&mut self, _: &str, _: &str, _: &str) -> Result<String> {
            Ok("queued".into())
        }
        fn recover_transient(&mut self, _: &str, _: StuckCause) -> Result<String> {
            self.attempts += 1;
            if self.fail {
                Err(fail("uncertain"))
            } else {
                Ok("continued".into())
            }
        }
    }
    fn report(kind: &str, since: f64) -> Value {
        json!({"episodes": [{"agent":"worker", "kind":kind, "since":since}]})
    }
    #[test]
    fn bounded_backoff_survives_restart_and_never_retries_uncertainty() {
        let fixture = crate::subagents::tests::Fixture::new();
        let mut options = super::super::tests::options();
        options.state_dir = Some(fixture.root.join("watch"));
        options.auto_recover = vec!["worker".into()];
        let mut sink = Sink {
            attempts: 0,
            fail: false,
        };
        let r = report("model-capacity", 0.0);
        for (now, expected) in [
            (0.0, 0),
            (59.0, 0),
            (60.0, 1),
            (179.0, 1),
            (180.0, 2),
            (419.0, 2),
            (420.0, 3),
            (10000.0, 3),
        ] {
            recover_due(&r, &mut sink, now, &options).unwrap();
            assert_eq!(sink.attempts, expected, "at {now}");
        }
        // A new episode can retry, but one uncertain attempt permanently disarms it.
        let r = report("api-rate-limit", 20000.0);
        sink.fail = true;
        recover_due(&r, &mut sink, 20000.0, &options).unwrap();
        recover_due(&r, &mut sink, 20060.0, &options).unwrap();
        assert_eq!(sink.attempts, 4);
        recover_due(
            &report("model-capacity", 30000.0),
            &mut sink,
            40000.0,
            &options,
        )
        .unwrap();
        assert_eq!(sink.attempts, 4);
    }
    #[test]
    fn opt_in_dry_run_and_nonrecoverable_mutations() {
        let fixture = crate::subagents::tests::Fixture::new();
        let mut options = super::super::tests::options();
        options.state_dir = Some(fixture.root.join("watch"));
        let mut sink = Sink {
            attempts: 0,
            fail: false,
        };
        for kind in [
            "model-capacity",
            "api-rate-limit",
            "out-of-quota",
            "safety-refusal",
            "unsafe-command",
            "goal-stalled",
            "approval-prompt",
        ] {
            recover_due(&report(kind, 0.0), &mut sink, 0.0, &options).unwrap();
            recover_due(&report(kind, 0.0), &mut sink, 9999.0, &options).unwrap();
        }
        assert_eq!(sink.attempts, 0);
        options.auto_recover = vec!["worker".into()];
        for kind in [
            "out-of-quota",
            "safety-refusal",
            "unsafe-command",
            "goal-stalled",
            "approval-prompt",
        ] {
            recover_due(&report(kind, 0.0), &mut sink, 0.0, &options).unwrap();
            recover_due(&report(kind, 0.0), &mut sink, 9999.0, &options).unwrap();
        }
        options.check_only = true;
        recover_due(&report("model-capacity", 0.0), &mut sink, 0.0, &options).unwrap();
        recover_due(&report("model-capacity", 0.0), &mut sink, 9999.0, &options).unwrap();
        assert_eq!(sink.attempts, 0);
        assert!(!options.state_dir.unwrap().exists());
    }
}
