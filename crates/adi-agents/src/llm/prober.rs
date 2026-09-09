//! Finding out that a backend is back — **in the background**, never on somebody's turn.
//!
//! This is requirement 4, and it is a requirement about *who pays*. A hold says "spent until 14:00".
//! With nothing here, the first chat to start a turn after 14:00 is the one that finds out whether
//! that was true: if the provider is still out, that person waits through a doomed request, reads an
//! error, and watches their conversation move again. So a held backend is tested by a sweep on a
//! timer instead, with one throwaway request, and the answer is written into the shared hold store
//! before any conversation asks.
//!
//! ```text
//! due()          every hold whose deadline has passed
//!   -> probe     one tiny request — no tools, no transcript, no session
//!        back    release the hold; the next turn simply works
//!        out     extend it, doubling, so nobody asks again for twice as long
//! ```
//!
//! # What it will not do
//!
//! It never probes a backend that declares no [`Probe`](crate::llm::Probe), and it never invents
//! one. A probe is a real billed request against somebody's subscription, run on a timer and
//! forever, so it is opt-in per backend rather than something that starts happening when this ships.
//!
//! It can also only *reach* providers ADI calls itself — the `harness:adi` runtime. A hold on a
//! vendor-CLI backend is reported [`Unreachable`](Verdict::Unreachable) and left to expire on its
//! own deadline, which is the honest outcome: the provider's stated reset time is trusted rather
//! than verified. Nothing is ever marked recovered on a guess.

use std::time::Duration;

use adi_config::{Config, now_unix};

use crate::error::{Error, Result};
use crate::llm::backend::{LlmBackend, LlmBackendManifest, LlmBackends};
use crate::llm::classify::classify;
use crate::llm::holds::{Hold, HoldKey, Holds};
use crate::llm::settings::LlmSettings;

/// What a probe found, after the hold store has been told about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// It answered. The hold is gone, and the next turn will simply use it.
    Back,
    /// Still limited, in the provider's own words. The hold has been pushed out — doubling, so a
    /// backend that is out for the day is not asked every four hours forever.
    StillOut {
        /// When it may be tried again now, in unix seconds.
        until: u64,
        /// What it said this time.
        reason: String,
    },
    /// It failed for a reason that is not a limit — a dead key, a DNS failure, a 500. Left held and
    /// reported, because a probe that cannot say why it failed must not report recovery.
    Failed {
        /// What went wrong, verbatim.
        error: String,
    },
    /// Nothing here can ask this backend anything: it declares no probe, its definition is gone, or
    /// it answers through a vendor CLI rather than a provider ADI calls. The hold stands and expires
    /// on its own deadline.
    Unreachable {
        /// Which of those it is.
        reason: String,
    },
}

/// One hold, and what the sweep made of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// The hold's key — the credential, and the model unless it is login-scoped.
    pub key: HoldKey,
    /// The backend the probe went through, when one could be found to send it.
    pub backend: Option<String>,
    pub verdict: Verdict,
}

/// The prober: a sweep over the shared hold store, run on a timer by whoever supervises it.
///
/// It holds no state of its own beyond where the store is — every sweep re-reads both the holds and
/// the backend definitions — so it is safe to run from a service, from the CLI by hand and from a
/// test, all against the same database, and an operator editing a backend mid-sweep is picked up on
/// the next pass rather than baked in at startup.
#[derive(Debug, Clone)]
pub struct Prober {
    config: Config,
}

impl Prober {
    /// A prober over a config root's `llm/` module.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self { config }
    }

    /// Check every hold whose time is up, once, and report what became of each.
    ///
    /// An empty result is the ordinary case, and the good one: nothing is held, or nothing held is
    /// due yet.
    ///
    /// # Errors
    /// Store errors from reading the holds or the backend definitions. A probe that *fails* is not
    /// an error — it is a [`Verdict`], because one unreachable backend must not stop the sweep
    /// testing the others.
    pub fn sweep(&self) -> Result<Vec<Checked>> {
        let holds = Holds::with_config(&self.config);
        let due = holds.due()?;
        if due.is_empty() {
            return Ok(Vec::new());
        }
        let backends = LlmBackends::with_config(self.config.clone()).list()?;
        due.into_iter()
            .map(|hold| check(&holds, &backends, &hold))
            .collect()
    }

    /// Probe one named backend, whatever its hold says — what somebody runs to check a backend they
    /// have just written, and what `adi-mono llm probe <id>` does.
    ///
    /// It answers about the backend rather than about a hold, so it writes nothing: releasing a hold
    /// this backend does not own would be a guess about somebody else's credential.
    ///
    /// # Errors
    /// [`Error::NotFound`] when there is no such backend.
    pub fn probe_backend(&self, id: &str) -> Result<Verdict> {
        let backend = LlmBackends::with_config(self.config.clone())
            .get(id)?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        Ok(match ask(&backend.manifest) {
            Outcome::Answered => Verdict::Back,
            Outcome::Limited { hold_for, reason } => Verdict::StillOut {
                until: now_unix().saturating_add(hold_for),
                reason,
            },
            Outcome::Failed { error } => Verdict::Failed { error },
            Outcome::Unreachable { reason } => Verdict::Unreachable { reason },
        })
    }

    /// How long to wait before the next sweep, or `None` when probing is switched off entirely
    /// (`probe_every = 0`, which leaves every hold to expire on its own deadline).
    ///
    /// Re-read from `llm/settings.toml` on every call rather than captured at startup, so changing
    /// the interval — or switching probing off — takes effect within one cycle instead of at the
    /// next restart. It lives here rather than in each caller's loop because there are two of those
    /// loops (the app's worker and `llm probe --watch`), and a setting one of them honoured and the
    /// other ignored would be worse than no setting at all.
    #[must_use]
    pub fn interval(&self) -> Option<Duration> {
        let every = LlmSettings::open(&self.config).probe_every;
        (every > 0).then(|| Duration::from_secs(every))
    }

}

/// One hold: find something that can ask on its behalf, ask, and write down the answer.
///
/// A free function rather than a method because it needs no config of its own — it is handed the
/// store and the backends the sweep already read, so a sweep of forty holds opens neither of them
/// forty times.
fn check(holds: &Holds, backends: &[LlmBackend], hold: &Hold) -> Result<Checked> {
    // A login-scoped hold names no model, so anything on that credential can answer for it; a
    // model-scoped one has to be asked on its own model, since that is what is limited.
    let found = backends.iter().find(|backend| {
        backend.manifest.credential() == hold.key.credential
            && (hold.key.model.is_empty() || backend.manifest.model == hold.key.model)
    });
    let Some(backend) = found else {
        return Ok(Checked {
            key: hold.key.clone(),
            backend: None,
            verdict: Verdict::Unreachable {
                reason: "no backend on record answers for this credential any more".into(),
            },
        });
    };

    let verdict = match ask(&backend.manifest) {
        Outcome::Answered => {
            holds.release(&hold.key)?;
            Verdict::Back
        }
        Outcome::Limited { hold_for, reason } => {
            // The fresh wait is the *base*, and `extend` doubles it by the attempt count it keeps —
            // so a provider repeating "try again in 4h" at every sweep does not buy a probe every
            // four hours forever.
            let until = holds
                .extend(&hold.key, hold_for)?
                .map_or_else(|| now_unix().saturating_add(hold_for), |hold| hold.until);
            Verdict::StillOut { until, reason }
        }
        // A hold nobody could test keeps the deadline it already has. Extending it because DNS was
        // down would punish a backend for the network; releasing it would tell sixteen agents that
        // a spent subscription is ready.
        Outcome::Failed { error } => Verdict::Failed { error },
        Outcome::Unreachable { reason } => Verdict::Unreachable { reason },
    };
    Ok(Checked {
        key: hold.key.clone(),
        backend: Some(backend.id.clone()),
        verdict,
    })
}

/// What one probe found, before anything has been written down. Separate from [`Verdict`] so the
/// judgement can be made — and tested — without a hold store, and so the caller that owns the holds
/// is the only thing that changes them.
enum Outcome {
    Answered,
    Limited { hold_for: u64, reason: String },
    Failed { error: String },
    Unreachable { reason: String },
}

/// Send one probe and read the result against this backend's own limit rules.
fn ask(manifest: &LlmBackendManifest) -> Outcome {
    let Some(probe) = manifest.probe.as_ref() else {
        return Outcome::Unreachable {
            reason: "this backend declares no probe, so it is never asked".into(),
        };
    };
    if manifest.runtime != crate::Backend::HarnessAdi {
        return Outcome::Unreachable {
            reason: format!(
                "{} answers through a vendor CLI rather than a provider ADI calls, so this hold is \
                 left to expire on the deadline the provider gave",
                manifest.runtime
            ),
        };
    }
    let model = probe
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or(&manifest.model);

    match crate::backends::harness::adi_loop::probe(&manifest.arguments(), model, &probe.prompt) {
        Ok(_) => Outcome::Answered,
        Err(error) => {
            let text = error.to_string();
            // Read with the same rules a real turn is read with, so "still limited" means exactly
            // what it meant when the hold was written, rather than whatever a second heuristic here
            // would have decided.
            let found = classify(&text, &manifest.limit_rules, None, now_unix());
            if found.class.holds() {
                Outcome::Limited {
                    hold_for: found.hold_for,
                    reason: found.evidence,
                }
            } else {
                Outcome::Failed { error: text }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::backend::{HoldScope, LimitClass, LimitRule, Probe, Resume};

    fn scratch(tag: &str) -> Config {
        let root = std::env::temp_dir().join(format!(
            "adi-agents-prober-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&root);
        Config::with_root(root)
    }

    fn held(key: HoldKey, until: u64) -> Hold {
        Hold {
            key,
            class: LimitClass::Quota,
            until,
            reason: "usage limit reached".into(),
            set_by: "solver/c1".into(),
            attempts: 0,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// A vendor-CLI backend: the shape most of the migrated ones will have.
    fn cli_backend(probe: Option<Probe>) -> LlmBackendManifest {
        LlmBackendManifest {
            runtime: "harness:claude-sdk".into(),
            model: "claude-opus-5".into(),
            provider: Some("anthropic".into()),
            probe,
            limit_rules: vec![LimitRule {
                pattern: "(?i)usage limit reached".into(),
                class: LimitClass::Quota,
                scope: HoldScope::Model,
                resume: Resume::Fixed,
                fixed: Some("4h".into()),
            }],
            ..LlmBackendManifest::default()
        }
    }

    /// The opt-in, and it is the first thing checked: a backend that declares no probe is never
    /// asked anything, however long it has been held. A probe is a billed request on somebody's
    /// subscription running forever on a timer — it does not start happening by itself.
    #[test]
    fn a_backend_that_declares_no_probe_is_never_asked() {
        assert!(matches!(
            ask(&cli_backend(None)),
            Outcome::Unreachable { reason } if reason.contains("declares no probe")
        ));
    }

    /// The honest limit of this version. A vendor-CLI backend cannot be asked a two-token question
    /// over HTTP, so its hold is reported unreachable and left to expire on the provider's own
    /// stated deadline — never silently marked recovered.
    #[test]
    fn a_vendor_cli_backend_is_reported_unreachable_rather_than_guessed_at() {
        assert!(matches!(
            ask(&cli_backend(Some(Probe::default()))),
            Outcome::Unreachable { reason } if reason.contains("vendor CLI")
        ));
    }

    /// A sweep only looks at holds whose time is up. One still blocking is somebody else's business
    /// and must not cost a request.
    #[test]
    fn a_hold_that_is_still_blocking_is_not_probed() {
        let config = scratch("not-due");
        Holds::with_config(&config)
            .hold(&held(
                HoldKey::new("anthropic", "claude-opus-5"),
                now_unix() + 3_600,
            ))
            .expect("hold");

        assert!(
            Prober::with_config(config).sweep().expect("sweep").is_empty(),
            "nothing is due, so nothing is asked",
        );
    }

    /// A hold whose backend has since been deleted still comes up in the sweep — and is reported,
    /// not skipped. It is exactly the state somebody needs to see: a credential being held out of
    /// every agent's chain by a definition nobody has any more.
    #[test]
    fn a_hold_whose_backend_is_gone_is_reported_rather_than_silently_skipped() {
        let config = scratch("orphan");
        Holds::with_config(&config)
            .hold(&held(HoldKey::new("vanished", "some-model"), 1))
            .expect("hold");

        let swept = Prober::with_config(config).sweep().expect("sweep");
        assert_eq!(swept.len(), 1);
        assert_eq!(swept[0].backend, None);
        assert!(
            matches!(&swept[0].verdict, Verdict::Unreachable { reason } if reason.contains("no backend")),
            "{:?}",
            swept[0].verdict,
        );
    }

    /// A due hold on a backend that cannot be probed keeps its hold: the sweep reports what it found
    /// and changes nothing, because an unverifiable backend is neither back nor further out.
    #[test]
    fn an_unreachable_backend_keeps_the_hold_it_already_had() {
        let config = scratch("keep");
        LlmBackends::with_config(config.clone())
            .save("anthropic", cli_backend(Some(Probe::default())))
            .expect("save");

        let holds = Holds::with_config(&config);
        holds
            .hold(&held(
                HoldKey::new(cli_backend(None).credential(), "claude-opus-5"),
                1,
            ))
            .expect("hold");

        let swept = Prober::with_config(config).sweep().expect("sweep");
        assert_eq!(swept.len(), 1);
        assert_eq!(swept[0].backend.as_deref(), Some("anthropic"));
        assert_eq!(
            holds.all().expect("read").len(),
            1,
            "the hold stands — nothing was verified, so nothing was released",
        );
    }

    /// A login-scoped hold names no model, so anything on that credential can answer for it. Without
    /// this, an `auth` hold would be unprobeable forever and the login would stay dark until
    /// somebody cleared it by hand.
    #[test]
    fn a_login_scoped_hold_is_answered_by_any_backend_on_that_credential() {
        let config = scratch("login-scope");
        LlmBackends::with_config(config.clone())
            .save("anthropic", cli_backend(Some(Probe::default())))
            .expect("save");

        Holds::with_config(&config)
            .hold(&held(HoldKey::login(cli_backend(None).credential()), 1))
            .expect("hold");

        let swept = Prober::with_config(config).sweep().expect("sweep");
        assert_eq!(
            swept[0].backend.as_deref(),
            Some("anthropic"),
            "a hold on the whole login is probed through whatever runs on it",
        );
    }

    /// A model-scoped hold is probed on *its* model. An Opus quota must not be tested by asking the
    /// Sonnet backend on the same subscription — that answers, and would release a hold on a model
    /// that is still spent.
    #[test]
    fn a_model_scoped_hold_is_not_answered_by_a_sibling_on_another_model() {
        let config = scratch("model-scope");
        LlmBackends::with_config(config.clone())
            .save("sonnet", LlmBackendManifest {
                model: "claude-sonnet-5".into(),
                ..cli_backend(Some(Probe::default()))
            })
            .expect("save");

        Holds::with_config(&config)
            .hold(&held(
                HoldKey::new(cli_backend(None).credential(), "claude-opus-5"),
                1,
            ))
            .expect("hold");

        let swept = Prober::with_config(config).sweep().expect("sweep");
        assert_eq!(
            swept[0].backend, None,
            "the Sonnet row shares the login but cannot speak for the Opus quota",
        );
    }

    /// The probe's model overrides the backend's, which is the whole point of declaring one: a
    /// subscription is checked with the cheapest thing it can reach, not with the model somebody is
    /// paying for by the token.
    #[test]
    fn a_probe_may_name_a_cheaper_model_than_the_backend_runs() {
        let manifest = LlmBackendManifest {
            probe: Some(Probe {
                model: Some("claude-haiku-4-5-20251001".into()),
                prompt: "ok".into(),
            }),
            ..cli_backend(None)
        };
        let probe = manifest.probe.as_ref().expect("a probe");
        assert_ne!(probe.model.as_deref(), Some(manifest.model.as_str()));
        assert_eq!(
            probe.prompt, "ok",
            "a couple of tokens, and the reply discarded",
        );
    }

    /// Probing a backend by name answers about the backend and writes nothing. It is a diagnostic
    /// somebody runs by hand, not a second path into the shared hold store.
    #[test]
    fn probing_one_backend_by_name_touches_no_hold() {
        let config = scratch("by-name");
        LlmBackends::with_config(config.clone())
            .save("anthropic", cli_backend(Some(Probe::default())))
            .expect("save");
        let holds = Holds::with_config(&config);
        holds
            .hold(&held(
                HoldKey::new(cli_backend(None).credential(), "claude-opus-5"),
                1,
            ))
            .expect("hold");

        let prober = Prober::with_config(config);
        assert!(matches!(
            prober.probe_backend("anthropic").expect("probe"),
            Verdict::Unreachable { .. }
        ));
        assert_eq!(holds.all().expect("read").len(), 1);
        assert!(prober.probe_backend("nobody").is_err(), "and it says so");
    }

    /// `probe_every = 0` is an off switch, not a zero-second timer — and both watchers ask this one
    /// method, so neither can turn it into a sweep a second.
    #[test]
    fn probing_can_be_switched_off_entirely() {
        let config = scratch("interval");
        let prober = Prober::with_config(config.clone());
        assert_eq!(
            prober.interval(),
            Some(Duration::from_secs(
                crate::llm::settings::DEFAULT_PROBE_EVERY
            )),
        );

        LlmSettings {
            ask_on_switch: false,
            probe_every: 0,
        }
        .save(&config.module(crate::llm::LLM_MODULE))
        .expect("save");
        assert_eq!(
            prober.interval(),
            None,
            "off means holds expire on their own deadline, not that they are checked constantly",
        );
    }
}
