use super::*;

#[test]
fn first_snapshot_takes_idle_agents_as_seen() {
    let mut presentation = EndpointAgentPresentation::default();
    presentation.receive_completions(None, completions("endpoint-boot", 1, Some(4)));
    let mut snapshot = snapshot(AgentStatus::Idle, 4, 1);

    presentation.project_snapshot(&mut snapshot);

    assert_eq!(snapshot.agents[0].agent_status, AgentStatus::Idle);
    assert!(!presentation.awaits_user(&snapshot.agents[0]));
}

#[test]
fn a_finished_agent_on_screen_awaits_the_user_until_the_server_counts_it_as_seen() {
    let delay = std::time::Duration::from_secs(1);
    let mut presentation = EndpointAgentPresentation {
        seen_delay: delay,
        ..Default::default()
    };
    let mut working = snapshot(AgentStatus::Working, 4, 1);
    presentation.project_snapshot(&mut working);
    // The server reports it done: it doesn't count it as seen yet either.
    let mut finished = snapshot(AgentStatus::Done, 5, 2);
    presentation.project_snapshot(&mut finished);
    assert!(!presentation.acknowledge_surface(&mut finished, &surface(2), Some(true)));
    // The server decides, so the window's timer has nothing to wait for.
    assert!(!presentation.waiting);

    // On screen past the delay by the window's clock, but not yet by the server's.
    for since in presentation.on_screen_since.values_mut() {
        *since -= delay;
    }
    assert!(!presentation.acknowledge_surface(&mut finished, &surface(2), Some(true)));
    assert!(presentation.awaits_user(&finished.agents[0]));

    // The server counts it as seen, and the window's red count goes with its green frame,
    // with the snapshot that says so: no look is needed.
    let mut seen = snapshot(AgentStatus::Idle, 5, 3);
    presentation.project_snapshot(&mut seen);
    assert_eq!(seen.agents[0].agent_status, AgentStatus::Idle);
    assert!(!presentation.awaits_user(&seen.agents[0]));
    assert!(!presentation.acknowledge_surface(&mut seen, &surface(3), Some(true)));
}

#[test]
fn without_a_delay_a_finished_agent_counts_as_seen_as_soon_as_its_pane_shows() {
    let mut presentation = EndpointAgentPresentation::default();
    let mut working = snapshot(AgentStatus::Working, 4, 1);
    presentation.project_snapshot(&mut working);
    // The window's look and the server's land together: it doesn't wait for the server.
    let mut finished = snapshot(AgentStatus::Done, 5, 2);
    presentation.project_snapshot(&mut finished);
    assert!(presentation.awaits_user(&finished.agents[0]));

    assert!(presentation.acknowledge_surface(&mut finished, &surface(2), Some(true)));
    assert!(!presentation.awaits_user(&finished.agents[0]));
    assert!(!presentation.waiting);
}

#[test]
fn a_question_awaits_the_user_until_its_pane_stays_on_screen_for_the_delay() {
    let delay = std::time::Duration::from_secs(1);
    let mut presentation = EndpointAgentPresentation {
        seen_delay: delay,
        ..Default::default()
    };
    let mut working = snapshot(AgentStatus::Working, 4, 1);
    presentation.project_snapshot(&mut working);
    let mut asking = snapshot(AgentStatus::Blocked, 5, 2);
    presentation.project_snapshot(&mut asking);

    // Just on screen: not looked at yet, and the window's timer has to look again.
    assert!(!presentation.acknowledge_surface(&mut asking, &surface(2), Some(true)));
    assert!(presentation.awaits_user(&asking.agents[0]));
    assert!(presentation.waiting);

    // The window in the background starts the wait over.
    assert!(!presentation.acknowledge_surface(&mut asking, &surface(2), Some(false)));
    assert!(presentation.on_screen_since.is_empty());
    assert!(!presentation.waiting);

    presentation.acknowledge_surface(&mut asking, &surface(2), Some(true));
    for since in presentation.on_screen_since.values_mut() {
        *since -= delay;
    }
    assert!(presentation.acknowledge_surface(&mut asking, &surface(2), Some(true)));
    assert!(!presentation.awaits_user(&asking.agents[0]));
    assert!(!presentation.waiting);
}

#[test]
fn an_agent_that_finishes_on_screen_counts_as_seen_when_the_server_counts_it() {
    let mut presentation = EndpointAgentPresentation {
        seen_delay: std::time::Duration::from_secs(1),
        ..Default::default()
    };
    let mut working = snapshot(AgentStatus::Working, 4, 1);
    presentation.project_snapshot(&mut working);
    presentation.acknowledge_surface(&mut working, &surface(1), Some(true));

    // It finishes a moment after its tab came on screen, and the server counts it as seen.
    let mut finished = snapshot(AgentStatus::Idle, 5, 2);
    presentation.project_snapshot(&mut finished);
    assert!(presentation.acknowledge_surface(&mut finished, &surface(2), Some(true)));
    let mut elsewhere = surface(2);
    elsewhere.panes.clear();
    presentation.acknowledge_surface(&mut finished, &elsewhere, Some(true));
    assert!(!presentation.awaits_user(&finished.agents[0]));
}

#[test]
fn the_timer_counts_a_question_as_seen_once_it_has_waited_and_then_stops_looking() {
    use crate::client::shell::{ClientEndpointId, ClientShellConfig, ClientShellState};
    let mut shell = ClientShellState::new(ClientShellConfig::from_config(
        &crate::config::Config::default(),
    ));
    let id = ClientEndpointId::Local;
    shell.outer_focused = Some(true);
    shell.set_endpoint_snapshot_for_generation(
        &id,
        4,
        Box::new(snapshot(AgentStatus::Working, 4, 1)),
    );
    shell.set_pane_surface(surface(1));
    fn presentation(shell: &mut ClientShellState) -> &mut EndpointAgentPresentation {
        &mut shell
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == ClientEndpointId::Local)
            .unwrap()
            .agent_presentation
    }
    fn awaits_user(shell: &mut ClientShellState) -> bool {
        let agent = shell.snapshot.as_ref().unwrap().agents[0].clone();
        presentation(shell).awaits_user(&agent)
    }

    // A question: on screen, but not for long enough yet.
    shell.set_endpoint_snapshot_for_generation(
        &id,
        4,
        Box::new(snapshot(AgentStatus::Blocked, 5, 2)),
    );
    shell.set_pane_surface(surface(2));
    assert!(presentation(&mut shell).waiting);
    assert!(!shell.tick_seen());
    assert!(awaits_user(&mut shell));

    for since in presentation(&mut shell).on_screen_since.values_mut() {
        *since -= std::time::Duration::from_secs(1);
    }
    assert!(shell.tick_seen());
    assert!(!awaits_user(&mut shell));
    // Nothing on screen waits any more, so the timer doesn't look again.
    assert!(!presentation(&mut shell).waiting);
    assert!(!shell.tick_seen());

    // The agent works and finishes in the pane, long on screen: it waits for the server, not
    // the timer.
    shell.set_endpoint_snapshot_for_generation(
        &id,
        4,
        Box::new(snapshot(AgentStatus::Working, 6, 3)),
    );
    shell.set_pane_surface(surface(3));
    shell.set_endpoint_snapshot_for_generation(&id, 4, Box::new(snapshot(AgentStatus::Done, 7, 4)));
    shell.set_pane_surface(surface(4));
    assert!(awaits_user(&mut shell));
    assert!(!presentation(&mut shell).waiting);
    assert!(!shell.tick_seen());

    // In the background, the wait starts over: nothing waits until the window looks again.
    shell.outer_focused = Some(false);
    assert!(!shell.tick_seen());
    assert!(presentation(&mut shell).on_screen_since.is_empty());
    assert!(!presentation(&mut shell).waiting);
}

#[test]
fn the_timer_leaves_a_snapshot_cached_while_reconnecting_alone() {
    use crate::client::shell::{ClientEndpointId, ClientShellConfig, ClientShellState};
    let mut shell = ClientShellState::new(ClientShellConfig::from_config(
        &crate::config::Config::default(),
    ));
    let id = ClientEndpointId::Local;
    // A question on screen, so the timer looks again.
    shell.set_endpoint_snapshot_for_generation(
        &id,
        4,
        Box::new(snapshot(AgentStatus::Blocked, 4, 1)),
    );
    shell.set_pane_surface(surface(1));
    let endpoint = shell
        .endpoints
        .iter_mut()
        .find(|endpoint| endpoint.endpoint_id == id)
        .unwrap();
    assert!(endpoint.agent_presentation.waiting);
    for since in endpoint.agent_presentation.on_screen_since.values_mut() {
        *since -= std::time::Duration::from_secs(1);
    }
    // A new connection's first snapshot, same boot and revision, with a new question.
    shell.mark_endpoint_disconnected(&id);
    shell.cache_endpoint_snapshot_inactive_for_generation(
        &id,
        5,
        Box::new(snapshot(AgentStatus::Blocked, 5, 1)),
    );

    shell.tick_seen();
    assert_eq!(
        shell.snapshot.as_ref().unwrap().agents[0].state_change_seq,
        4,
        "the old connection's frame must not take in the new connection's snapshot"
    );
    let endpoint = shell
        .endpoints
        .iter()
        .find(|endpoint| endpoint.endpoint_id == id)
        .unwrap();
    assert_eq!(endpoint.unseen_agents().count(), 1);
}

#[test]
fn the_window_in_the_background_while_reconnecting_starts_the_wait_over() {
    use crate::client::shell::{ClientEndpointId, ClientShellConfig, ClientShellState};
    let mut shell = ClientShellState::new(ClientShellConfig::from_config(
        &crate::config::Config::default(),
    ));
    let id = ClientEndpointId::Local;
    shell.outer_focused = Some(true);
    shell.set_endpoint_snapshot_for_generation(
        &id,
        4,
        Box::new(snapshot(AgentStatus::Working, 4, 7)),
    );
    shell.set_pane_surface(surface(7));
    for since in shell
        .endpoints
        .iter_mut()
        .find(|endpoint| endpoint.endpoint_id == id)
        .unwrap()
        .agent_presentation
        .on_screen_since
        .values_mut()
    {
        *since -= std::time::Duration::from_secs(2);
    }
    shell.mark_endpoint_disconnected(&id);
    // The new connection's first snapshot waits (the agent asked a question meanwhile).
    shell.cache_endpoint_snapshot_inactive_for_generation(
        &id,
        5,
        Box::new(snapshot(AgentStatus::Blocked, 5, 1)),
    );
    // In the background for a while (timer ticks), then back.
    shell.outer_focused = Some(false);
    shell.tick_seen();
    shell.outer_focused = Some(true);
    // The new connection's frame comes.
    shell.set_endpoint_snapshot_for_generation(
        &id,
        5,
        Box::new(snapshot(AgentStatus::Blocked, 5, 2)),
    );
    shell.set_pane_surface(surface(2));
    let endpoint = shell
        .endpoints
        .iter()
        .find(|endpoint| endpoint.endpoint_id == id)
        .unwrap();
    assert_eq!(
        endpoint.unseen_agents().count(),
        1,
        "on screen for no time since the window came back, yet counted as seen"
    );
}

#[test]
fn coming_back_to_the_window_while_reconnecting_leaves_the_cached_snapshot_alone() {
    use crate::client::shell::{ClientEndpointId, ClientShellConfig, ClientShellState};
    use crate::raw_input::RawInputEvent;
    // In the new connection's first snapshot: a question, which the window's own clock
    // decides; a finish the server counted as seen at once; and, without a delay, a finished
    // agent, which a look acknowledges at once.
    for (seen_delay, status) in [
        (std::time::Duration::from_secs(1), AgentStatus::Blocked),
        (std::time::Duration::from_secs(1), AgentStatus::Idle),
        (std::time::Duration::ZERO, AgentStatus::Done),
    ] {
        let mut shell = ClientShellState::new(ClientShellConfig::from_config(
            &crate::config::Config::default(),
        ));
        shell.config.seen_delay = seen_delay;
        let id = ClientEndpointId::Local;
        shell.handle_raw_events(vec![RawInputEvent::OuterFocusGained]);
        shell.set_endpoint_snapshot_for_generation(
            &id,
            4,
            Box::new(snapshot(AgentStatus::Working, 4, 1)),
        );
        shell.set_pane_surface(surface(1));
        for since in shell
            .endpoints
            .iter_mut()
            .find(|endpoint| endpoint.endpoint_id == id)
            .unwrap()
            .agent_presentation
            .on_screen_since
            .values_mut()
        {
            *since -= std::time::Duration::from_secs(1);
        }
        shell.mark_endpoint_disconnected(&id);
        shell.cache_endpoint_snapshot_inactive_for_generation(
            &id,
            5,
            Box::new(snapshot(status, 5, 1)),
        );
        shell.handle_raw_events(vec![RawInputEvent::OuterFocusLost]);
        shell.tick_seen();
        shell.handle_raw_events(vec![RawInputEvent::OuterFocusGained]);
        assert_eq!(
            shell.snapshot.as_ref().unwrap().agents[0].state_change_seq,
            4,
            "the old connection's frame must not take in the new connection's snapshot \
             ({status:?}, delay {seen_delay:?})"
        );
    }
}

#[test]
fn the_servers_verdict_on_a_finished_agent_counts_in_the_background_and_off_screen() {
    let mut elsewhere = surface(3);
    elsewhere.panes.clear();
    // The window went to the background, or on to another tab, just before the verdict came.
    for (outer_focused, look) in [(Some(false), surface(3)), (Some(true), elsewhere)] {
        let mut presentation = EndpointAgentPresentation {
            seen_delay: std::time::Duration::from_secs(1),
            ..Default::default()
        };
        let mut working = snapshot(AgentStatus::Working, 4, 1);
        presentation.project_snapshot(&mut working);
        let mut finished = snapshot(AgentStatus::Done, 5, 2);
        presentation.project_snapshot(&mut finished);
        presentation.acknowledge_surface(&mut finished, &surface(2), Some(true));
        assert!(presentation.awaits_user(&finished.agents[0]));

        let mut seen = snapshot(AgentStatus::Idle, 5, 3);
        presentation.project_snapshot(&mut seen);
        presentation.acknowledge_surface(&mut seen, &look, outer_focused);

        assert_eq!(seen.agents[0].agent_status, AgentStatus::Idle);
        assert!(!presentation.awaits_user(&seen.agents[0]));
    }
}

#[test]
fn finished_agents_and_questions_await_the_user_until_their_pane_is_on_screen() {
    let mut presentation = EndpointAgentPresentation::default();
    let mut baseline = snapshot(AgentStatus::Blocked, 4, 1);
    presentation.project_snapshot(&mut baseline);
    assert!(
        presentation.awaits_user(&baseline.agents[0]),
        "a question already open when the window opens waits for the user too"
    );
    presentation.acknowledge_surface(&mut baseline, &surface(1), Some(true));
    assert!(!presentation.awaits_user(&baseline.agents[0]));

    let mut asking = snapshot(AgentStatus::Blocked, 5, 2);
    presentation.project_snapshot(&mut asking);
    assert!(presentation.awaits_user(&asking.agents[0]));
    presentation.acknowledge_surface(&mut asking, &surface(2), Some(true));
    assert!(!presentation.awaits_user(&asking.agents[0]));

    let mut working = snapshot(AgentStatus::Working, 6, 3);
    presentation.project_snapshot(&mut working);
    assert!(!presentation.awaits_user(&working.agents[0]));
    let mut finished = snapshot(AgentStatus::Idle, 7, 4);
    presentation.project_snapshot(&mut finished);
    assert_eq!(finished.agents[0].agent_status, AgentStatus::Done);
    assert!(presentation.awaits_user(&finished.agents[0]));
    presentation.acknowledge_surface(&mut finished, &surface(4), Some(true));
    assert!(!presentation.awaits_user(&finished.agents[0]));
}
