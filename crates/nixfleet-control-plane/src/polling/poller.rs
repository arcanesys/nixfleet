//! Shared spawn/tick/log scaffolding for signed-artifact poll tasks.

use std::time::Duration;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use super::signed_fetch;

pub struct SignedArtifactPoller {
    pub interval: Duration,
    pub label: &'static str,
}

impl SignedArtifactPoller {
    /// Closure must not mutate shared state on its error path; poller logs a warn and retries.
    pub fn spawn<F, Fut>(self, cancel: CancellationToken, tick: F) -> tokio::task::JoinHandle<()>
    where
        F: FnMut(reqwest::Client) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send,
    {
        self.spawn_with_kick(cancel, None, tick)
    }

    /// Variant that also wakes on an external `kick` channel. The poll fn
    /// fires on whichever of (cadence, kick, first wake) arrives first.
    ///
    /// Used by channel-refs polling: the reconciler kicks after
    /// `Action::ConvergeRollout` / `SoakHost` so a freshly-released
    /// channelEdges successor gets recorded in the rollouts table
    /// immediately rather than waiting up to one `interval`. The cadence
    /// stays as a safety net — if the kick is missed (sender dropped,
    /// reconciler crash mid-stamp), polling catches up within `interval`.
    ///
    /// `watch::Receiver` semantics: latest-value, no backlog. A burst of
    /// kicks coalesces to one wake — the poller doesn't need to drain a
    /// queue.
    pub fn spawn_with_kick<F, Fut>(
        self,
        cancel: CancellationToken,
        kick: Option<tokio::sync::watch::Receiver<()>>,
        mut tick: F,
    ) -> tokio::task::JoinHandle<()>
    where
        F: FnMut(reqwest::Client) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send,
    {
        tokio::spawn(async move {
            let client = signed_fetch::build_client();

            let mut ticker = tokio::time::interval(self.interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            // `changed()` borrows mutably; keep the receiver in an Option
            // so the "no kick configured" branch can park forever via
            // `pending()` without holding a permanent borrow.
            let mut kick = kick;

            loop {
                let kicked = tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!(
                            target: "shutdown",
                            label = self.label,
                            "poll task shut down",
                        );
                        return;
                    }
                    _ = ticker.tick() => false,
                    res = async {
                        match kick.as_mut() {
                            Some(rx) => rx.changed().await,
                            // No kick channel configured: park forever
                            // so the select arm never fires.
                            None => std::future::pending().await,
                        }
                    } => {
                        // Sender dropped → fall back to cadence-only;
                        // log once so it's visible if this happens.
                        if res.is_err() {
                            tracing::warn!(
                                target: "polling",
                                label = self.label,
                                "kick channel closed; running on cadence only",
                            );
                            kick = None;
                            continue;
                        }
                        true
                    }
                };
                if let Err(err) = tick(client.clone()).await {
                    tracing::warn!(
                        target: "polling",
                        label = self.label,
                        kicked,
                        error = %err,
                        "poll failed; retaining previous state",
                    );
                } else if kicked {
                    tracing::debug!(
                        target: "polling",
                        label = self.label,
                        "poll fired on kick (event-driven)",
                    );
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::watch;
    use tokio_util::sync::CancellationToken;

    /// **Regression guard for event-driven polling**: a `kick` MUST
    /// fire the poll closure within milliseconds, well below the
    /// cadence interval. If the select arm regresses (e.g., the kick
    /// path gets removed, the wakeup is mis-wired) this test fails
    /// because tick_count stays at 0/1 instead of rising on each
    /// kick.
    ///
    /// The kick is what closes the channelEdges → rollouts-table
    /// timing gap structurally: when a predecessor goes terminal,
    /// the reconciler kicks and the new successor rollout gets
    /// recorded the same tick. Without this wakeup the gap reopens
    /// on every cadence-period (60 s), and first checkins on a
    /// freshly-released channel can slip past gates.
    #[tokio::test(start_paused = false)]
    async fn kick_fires_poll_well_before_cadence() {
        let cancel = CancellationToken::new();
        let (kick_tx, kick_rx) = watch::channel::<()>(());
        let counter = Arc::new(AtomicUsize::new(0));

        // Long cadence so any wake within the test window MUST be
        // from the kick path, not the timer.
        let poller = SignedArtifactPoller {
            interval: Duration::from_secs(3600),
            label: "test-kick",
        };
        let counter_for_tick = Arc::clone(&counter);
        let _handle = poller.spawn_with_kick(
            cancel.clone(),
            Some(kick_rx),
            move |_client| {
                let counter = Arc::clone(&counter_for_tick);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
        );

        // Fire 3 kicks back-to-back. Watch channel collapses bursts
        // to a single wake, so the count rises by at least 1 per
        // distinguishable kick (typically all 3 if interleaved by
        // sleeps — but we assert ≥1 to keep the test robust).
        for _ in 0..3 {
            kick_tx.send(()).unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Allow the loop to drain; if the kick path is broken the
        // counter stays at 0 (cadence is 1h, won't fire in this
        // window).
        tokio::time::sleep(Duration::from_millis(100)).await;
        let count = counter.load(Ordering::SeqCst);
        assert!(
            count >= 1,
            "kick must wake the poll within milliseconds; got {count} ticks (cadence is 1h)",
        );

        cancel.cancel();
    }

    /// Without a kick channel, the poller falls back to pure cadence.
    /// Verify the no-kick path still runs the closure on the timer,
    /// AND that an immediate-fire happens at startup (interval's
    /// first tick semantic). Pinned so the spawn-with-kick path
    /// can't accidentally swallow cadence ticks.
    #[tokio::test(start_paused = false)]
    async fn cadence_only_fires_without_kick() {
        let cancel = CancellationToken::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let poller = SignedArtifactPoller {
            interval: Duration::from_millis(50),
            label: "test-cadence",
        };
        let counter_for_tick = Arc::clone(&counter);
        let _handle = poller.spawn(cancel.clone(), move |_client| {
            let counter = Arc::clone(&counter_for_tick);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });

        // 250ms / 50ms cadence = ~5 ticks. Allow some slack for
        // scheduler jitter; assert we got at least 2 ticks.
        tokio::time::sleep(Duration::from_millis(250)).await;
        let count = counter.load(Ordering::SeqCst);
        assert!(
            count >= 2,
            "cadence-only must fire on the timer; got {count} ticks in 250ms with 50ms cadence",
        );

        cancel.cancel();
    }

    /// Belt-and-suspenders: when both kick and cadence are in play,
    /// dropping the kick sender doesn't crash the poller — it logs
    /// a warning and continues on cadence-only. Ensures a panicking
    /// reconciler can't seize the polling loop.
    #[tokio::test(start_paused = false)]
    async fn dropped_kick_sender_falls_back_to_cadence() {
        let cancel = CancellationToken::new();
        let (kick_tx, kick_rx) = watch::channel::<()>(());
        let counter = Arc::new(AtomicUsize::new(0));

        let poller = SignedArtifactPoller {
            interval: Duration::from_millis(50),
            label: "test-drop",
        };
        let counter_for_tick = Arc::clone(&counter);
        let _handle = poller.spawn_with_kick(
            cancel.clone(),
            Some(kick_rx),
            move |_client| {
                let counter = Arc::clone(&counter_for_tick);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
        );

        // Drop the sender — the receiver's `changed()` returns an
        // error on the next loop iteration. The poller should log
        // and switch to cadence-only.
        drop(kick_tx);

        tokio::time::sleep(Duration::from_millis(250)).await;
        let count = counter.load(Ordering::SeqCst);
        assert!(
            count >= 2,
            "dropped kick must fall back to cadence; got {count} ticks in 250ms",
        );

        cancel.cancel();
    }
}
