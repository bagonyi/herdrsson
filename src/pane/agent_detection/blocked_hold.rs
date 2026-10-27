//! Fork: an idle agent's blocked reading from a fallback rule must last before it is published.
//!
//! Fallback blocked rules (those without `visible_blocker`), such as Claude's
//! `legacy_no_prompt_blocker`, can match prompt-like words anywhere on screen. An idle Claude's
//! screen sometimes read blocked through that rule for up to about 3 s, probably while Claude
//! redrew it, then idle again. Blocked then idle counts as a finished turn, so a tab nobody had
//! touched for days got a finished mark and a notification. Only readings taken while the
//! published state is idle or unknown are held: real prompts that only a fallback rule catches,
//! such as Claude asking to edit a file, come while the agent is working and stay instant, as do
//! readings from rules marked `visible_blocker`.

use super::{DetectionPublishState, PendingIdleConfirmation};
use crate::detect::AgentState;

/// Blips in the user's logs lasted up to about 3 s.
pub(super) const AGENT_PENDING_BLOCKED_HOLD: std::time::Duration =
    std::time::Duration::from_secs(5);

impl PendingIdleConfirmation {
    /// Whether to keep back a fallback blocked reading on an idle agent. While it does, `active()`
    /// is true, so the detection loops reread the screen every 100 ms even when it hasn't changed.
    pub(super) fn should_hold_plain_blocked(
        &mut self,
        previous: DetectionPublishState,
        next: DetectionPublishState,
        agent_changed: bool,
        process_exited: bool,
        now: std::time::Instant,
    ) -> bool {
        let is_plain_blocked_on_idle =
            matches!(previous.state, AgentState::Idle | AgentState::Unknown)
                && next.state == AgentState::Blocked
                && !next.visible_blocker
                && !agent_changed
                && !process_exited;

        if !is_plain_blocked_on_idle {
            self.blocked_since = None;
            return false;
        }

        let since = *self.blocked_since.get_or_insert(now);
        if now.duration_since(since) >= AGENT_PENDING_BLOCKED_HOLD {
            self.blocked_since = None;
            return false;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        decide_detection_transition, should_skip_idle_screen_scan, DetectionTransitionDecision,
        DetectionTransitionInput, IdleScreenScanSkipInput, AGENT_PENDING_IDLE_RECHECK,
    };
    use super::*;
    use crate::detect::Agent;

    fn publish_state(state: AgentState) -> DetectionPublishState {
        DetectionPublishState {
            state,
            visible_idle: false,
            visible_blocker: false,
            visible_working: false,
        }
    }

    fn transition(
        pending: &mut PendingIdleConfirmation,
        previous: DetectionPublishState,
        next: DetectionPublishState,
        now: std::time::Instant,
    ) -> DetectionTransitionDecision {
        decide_detection_transition(
            DetectionTransitionInput {
                previous_publish: previous,
                next_publish: next,
                agent_changed: false,
                process_exited: false,
                stable_refresh_due: false,
                now,
            },
            pending,
        )
    }

    fn decide(
        pending: &mut PendingIdleConfirmation,
        previous: AgentState,
        next: AgentState,
        now: std::time::Instant,
    ) -> DetectionTransitionDecision {
        transition(pending, publish_state(previous), publish_state(next), now)
    }

    #[test]
    fn plain_blocked_on_an_idle_agent_publishes_only_once_it_has_lasted_the_hold() {
        for previous in [AgentState::Idle, AgentState::Unknown] {
            let now = std::time::Instant::now();
            let mut pending = PendingIdleConfirmation::default();

            assert_eq!(
                decide(&mut pending, previous, AgentState::Blocked, now),
                DetectionTransitionDecision::NoPublish
            );
            assert!(pending.active());
            // The published state is still idle, so an unchanged screen must not skip the rechecks.
            assert!(!should_skip_idle_screen_scan(IdleScreenScanSkipInput {
                state: previous,
                agent: Some(Agent::Claude),
                pending_idle_active: pending.active(),
                agent_changed: false,
                process_exited: false,
                current_detection_content_seq: Some(4),
                last_screen_scan_detection_content_seq: Some(4),
            }));
            assert_eq!(
                decide(
                    &mut pending,
                    previous,
                    AgentState::Blocked,
                    now + AGENT_PENDING_BLOCKED_HOLD - AGENT_PENDING_IDLE_RECHECK
                ),
                DetectionTransitionDecision::NoPublish
            );
            assert_eq!(
                decide(
                    &mut pending,
                    previous,
                    AgentState::Blocked,
                    now + AGENT_PENDING_BLOCKED_HOLD
                ),
                DetectionTransitionDecision::PublishNext
            );
            assert!(!pending.active());
        }
    }

    #[test]
    fn plain_blocked_blip_on_an_idle_agent_publishes_nothing() {
        let now = std::time::Instant::now();
        let blip = std::time::Duration::from_secs(3);
        let mut pending = PendingIdleConfirmation::default();

        assert_eq!(
            decide(&mut pending, AgentState::Idle, AgentState::Blocked, now),
            DetectionTransitionDecision::NoPublish
        );
        assert_eq!(
            decide(
                &mut pending,
                AgentState::Idle,
                AgentState::Blocked,
                now + blip
            ),
            DetectionTransitionDecision::NoPublish
        );
        assert_eq!(
            decide(
                &mut pending,
                AgentState::Idle,
                AgentState::Idle,
                now + blip + AGENT_PENDING_IDLE_RECHECK
            ),
            DetectionTransitionDecision::NoPublish
        );
        assert!(!pending.active());
    }

    #[test]
    fn plain_blocked_while_working_publishes_at_once() {
        let now = std::time::Instant::now();
        let mut pending = PendingIdleConfirmation::default();

        assert_eq!(
            decide(&mut pending, AgentState::Working, AgentState::Blocked, now),
            DetectionTransitionDecision::PublishNext
        );
        assert!(!pending.active());
    }

    #[test]
    fn a_blocker_losing_its_controls_is_not_held() {
        let now = std::time::Instant::now();
        let mut visible = publish_state(AgentState::Blocked);
        visible.visible_blocker = true;
        let mut pending = PendingIdleConfirmation::default();

        assert_eq!(
            transition(
                &mut pending,
                visible,
                publish_state(AgentState::Blocked),
                now
            ),
            DetectionTransitionDecision::PublishNext
        );
        assert!(!pending.active());
    }

    #[test]
    fn clear_restarts_the_hold() {
        let now = std::time::Instant::now();
        let mut pending = PendingIdleConfirmation::default();

        assert_eq!(
            decide(&mut pending, AgentState::Idle, AgentState::Blocked, now),
            DetectionTransitionDecision::NoPublish
        );
        pending.clear();
        assert!(!pending.active());
        assert_eq!(
            decide(
                &mut pending,
                AgentState::Idle,
                AgentState::Blocked,
                now + AGENT_PENDING_BLOCKED_HOLD
            ),
            DetectionTransitionDecision::NoPublish
        );
    }

    #[test]
    fn visible_blocker_agent_change_and_exit_skip_the_hold() {
        let now = std::time::Instant::now();
        let mut visible = publish_state(AgentState::Blocked);
        visible.visible_blocker = true;
        let mut pending = PendingIdleConfirmation::default();

        assert_eq!(
            transition(&mut pending, publish_state(AgentState::Idle), visible, now),
            DetectionTransitionDecision::PublishNext
        );
        for (agent_changed, process_exited) in [(true, false), (false, true)] {
            assert_eq!(
                decide_detection_transition(
                    DetectionTransitionInput {
                        previous_publish: publish_state(AgentState::Idle),
                        next_publish: publish_state(AgentState::Blocked),
                        agent_changed,
                        process_exited,
                        stable_refresh_due: false,
                        now,
                    },
                    &mut pending,
                ),
                DetectionTransitionDecision::PublishNext
            );
        }
        assert!(!pending.active());
    }
}
