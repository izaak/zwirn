//! Platform-independent scheduling for a foreground live session.
//!
//! The scheduler is an effect-emitting transition machine. Its future driver
//! owns monitoring, timers, and the synchronous reconciliation call; this
//! module only decides when those operations may begin and when the session is
//! finished. The driver must serialize inputs and handle each returned effect
//! before accepting dependent work.

use std::time::Duration;

#[cfg(target_os = "macos")]
mod macos;

const COALESCING_WINDOW: Duration = Duration::from_millis(50);

/// Work for the live-session driver to perform after a scheduler transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Effect {
    /// Run one complete reconciliation synchronously, then report completion.
    ///
    /// Emission is the run's start linearization point. The driver must execute
    /// this effect exactly once and call `Scheduler::reconciliation_finished`
    /// exactly once, even when shutdown is requested after emission.
    Reconcile,
    /// Promptly arrange the single active timer without resetting it later.
    ArmCoalescingWindow { after: Duration },
    /// Finish the foreground session.
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pending {
    None,
    Coalescing,
    Due,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    AwaitingMonitoring,
    Idle,
    Coalescing,
    Reconciling(Pending),
    Stopping,
    Stopped,
}

/// Deterministic live-session lifecycle and reconciliation scheduler.
///
/// Each method is one serialized input and returns at most one effect. The
/// scheduler enters `Reconciling` before returning `Effect::Reconcile`, so a
/// driver can execute that effect synchronously while hints and shutdown
/// requests continue to update the scheduler from their delivery context.
#[derive(Debug)]
pub(crate) struct Scheduler {
    state: State,
}

impl Scheduler {
    pub(crate) const fn new() -> Self {
        Self {
            state: State::AwaitingMonitoring,
        }
    }

    /// Records that monitoring can now deliver hints and requests the initial
    /// reconciliation immediately.
    pub(crate) fn monitoring_operational(&mut self) -> Option<Effect> {
        let State::AwaitingMonitoring = self.state else {
            return None;
        };
        self.state = State::Reconciling(Pending::None);
        Some(Effect::Reconcile)
    }

    /// Records one filesystem invalidation hint.
    pub(crate) fn filesystem_hint(&mut self) -> Option<Effect> {
        match self.state {
            State::Idle => {
                self.state = State::Coalescing;
                Some(Effect::ArmCoalescingWindow {
                    after: COALESCING_WINDOW,
                })
            }
            State::Reconciling(Pending::None) => {
                self.state = State::Reconciling(Pending::Coalescing);
                Some(Effect::ArmCoalescingWindow {
                    after: COALESCING_WINDOW,
                })
            }
            State::AwaitingMonitoring
            | State::Coalescing
            | State::Reconciling(_)
            | State::Stopping
            | State::Stopped => None,
        }
    }

    /// Records expiry of the single active coalescing window.
    pub(crate) fn coalescing_window_elapsed(&mut self) -> Option<Effect> {
        match self.state {
            State::Coalescing => {
                self.state = State::Reconciling(Pending::None);
                Some(Effect::Reconcile)
            }
            State::Reconciling(Pending::Coalescing) => {
                self.state = State::Reconciling(Pending::Due);
                None
            }
            _ => None,
        }
    }

    /// Records completion of the active synchronous reconciliation.
    ///
    /// Its outcome is intentionally absent: a normal result and a blocker have
    /// identical scheduling consequences and neither causes a timed retry.
    pub(crate) fn reconciliation_finished(&mut self) -> Option<Effect> {
        match self.state {
            State::Reconciling(Pending::None) => {
                self.state = State::Idle;
                None
            }
            State::Reconciling(Pending::Coalescing) => {
                self.state = State::Coalescing;
                None
            }
            State::Reconciling(Pending::Due) => {
                self.state = State::Reconciling(Pending::None);
                Some(Effect::Reconcile)
            }
            State::Stopping => {
                self.state = State::Stopped;
                Some(Effect::Stop)
            }
            _ => None,
        }
    }

    /// Requests orderly shutdown and discards all pending work.
    pub(crate) fn shutdown_requested(&mut self) -> Option<Effect> {
        match self.state {
            State::AwaitingMonitoring | State::Idle | State::Coalescing => {
                self.state = State::Stopped;
                Some(Effect::Stop)
            }
            State::Reconciling(_) => {
                self.state = State::Stopping;
                None
            }
            State::Stopping | State::Stopped => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitoring_gates_the_immediate_initial_reconciliation() {
        let mut scheduler = Scheduler::new();

        assert_eq!(scheduler.monitoring_operational(), Some(Effect::Reconcile));
    }

    #[test]
    fn the_first_pending_hint_owns_one_fixed_window() {
        let mut scheduler = idle_scheduler();

        let armed = scheduler.filesystem_hint().unwrap();
        let Effect::ArmCoalescingWindow { after } = armed else {
            panic!("the first hint should arm its coalescing window");
        };
        assert_eq!(after, COALESCING_WINDOW);
        assert_eq!(scheduler.filesystem_hint(), None);
        assert_eq!(scheduler.filesystem_hint(), None);

        assert_eq!(
            scheduler.coalescing_window_elapsed(),
            Some(Effect::Reconcile)
        );
    }

    #[test]
    fn a_hint_during_reconciliation_starts_one_serialized_follow_up() {
        let mut scheduler = Scheduler::new();
        assert_eq!(scheduler.monitoring_operational(), Some(Effect::Reconcile));

        let Effect::ArmCoalescingWindow { .. } = scheduler.filesystem_hint().unwrap() else {
            panic!("the first hint during the run should arm a window");
        };
        assert_eq!(scheduler.filesystem_hint(), None);
        assert_eq!(scheduler.coalescing_window_elapsed(), None);
        assert_eq!(
            scheduler.reconciliation_finished(),
            Some(Effect::Reconcile),
            "the follow-up cannot start before the active run finishes"
        );
        assert_eq!(scheduler.reconciliation_finished(), None);
    }

    #[test]
    fn a_hint_remains_pending_when_reconciliation_finishes_before_its_window() {
        let mut scheduler = Scheduler::new();
        assert_eq!(scheduler.monitoring_operational(), Some(Effect::Reconcile));
        let Effect::ArmCoalescingWindow { .. } = scheduler.filesystem_hint().unwrap() else {
            panic!("the hint during the run should arm a window");
        };

        assert_eq!(scheduler.reconciliation_finished(), None);
        assert_eq!(
            scheduler.coalescing_window_elapsed(),
            Some(Effect::Reconcile)
        );
    }

    #[test]
    fn reconciliation_completion_waits_for_a_later_hint_without_retrying() {
        let mut scheduler = Scheduler::new();
        assert_eq!(scheduler.monitoring_operational(), Some(Effect::Reconcile));

        assert_eq!(scheduler.reconciliation_finished(), None);
        let Effect::ArmCoalescingWindow { .. } = scheduler.filesystem_hint().unwrap() else {
            panic!("recovery should be driven by a later hint");
        };
        assert_eq!(
            scheduler.coalescing_window_elapsed(),
            Some(Effect::Reconcile)
        );
    }

    #[test]
    fn shutdown_without_an_active_reconciliation_stops_immediately() {
        let mut idle = idle_scheduler();
        assert_eq!(idle.shutdown_requested(), Some(Effect::Stop));
        assert_eq!(idle.filesystem_hint(), None);

        let mut coalescing = idle_scheduler();
        let Effect::ArmCoalescingWindow { .. } = coalescing.filesystem_hint().unwrap() else {
            panic!("the hint should arm a window");
        };
        assert_eq!(coalescing.shutdown_requested(), Some(Effect::Stop));
        assert_eq!(coalescing.coalescing_window_elapsed(), None);
    }

    #[test]
    fn shutdown_during_reconciliation_finishes_it_and_discards_pending_work() {
        let mut scheduler = Scheduler::new();
        assert_eq!(scheduler.monitoring_operational(), Some(Effect::Reconcile));
        let Effect::ArmCoalescingWindow { .. } = scheduler.filesystem_hint().unwrap() else {
            panic!("the hint should arm a window");
        };
        assert_eq!(scheduler.coalescing_window_elapsed(), None);

        assert_eq!(scheduler.shutdown_requested(), None);
        assert_eq!(scheduler.filesystem_hint(), None);
        assert_eq!(scheduler.reconciliation_finished(), Some(Effect::Stop));
        assert_eq!(scheduler.reconciliation_finished(), None);
    }

    fn idle_scheduler() -> Scheduler {
        let mut scheduler = Scheduler::new();
        assert_eq!(scheduler.monitoring_operational(), Some(Effect::Reconcile));
        assert_eq!(scheduler.reconciliation_finished(), None);
        scheduler
    }
}
