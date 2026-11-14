//! Which agent the pet follows (C# MainWindow.Companion.cs `RefreshCompanion` and the pager's actions): the most pressing one, a page
//! the user turned to while its agent is busy, a finished run's six seconds, and the card key's guard.

use mighty_companion::follow::{self, Agent, Follower};

fn agent(id: &str, status: &str, seen: i64) -> Agent {
    Agent { id: id.into(), status: status.into(), activity: None, started: Some(1), last_observed: Some(seen) }
}

#[test]
fn the_most_pressing_agent_is_followed_until_one_is_paged_to() {
    let mut follower = Follower::new(true);
    let agents = [agent("a", "running", 5), agent("b", "waiting", 1), agent("c", "running", 9)];
    let followed = follower.refresh(&agents, true, 0.0);
    assert_eq!(
        (followed.shown.as_deref(), followed.row, followed.position, followed.count),
        (Some("b"), 6, 2, 3),
        "a waiting agent comes first"
    );
    assert!(followed.bubble, "another agent opens the bubble");
    let pages: Vec<String> = agents.iter().map(|a| a.id.clone()).collect();
    follower.page(&pages, 1);
    assert_eq!(follower.refresh(&agents, true, 1.0).shown.as_deref(), Some("c"), "the next page is pinned");
    let done = [agent("a", "running", 5), agent("b", "waiting", 1), agent("c", "completed", 9)];
    assert_eq!(follower.refresh(&done, true, 2.0).shown.as_deref(), Some("b"), "a pin clears once its agent is no longer busy");
}

#[test]
fn a_finished_run_celebrates_for_six_seconds_then_puts_its_bubble_away() {
    let mut follower = Follower::new(true);
    let done = [agent("a", "completed", 1)];
    let first = follower.refresh(&done, true, 10.0);
    assert!(first.bubble && first.row == 4, "{first:?}");
    let later = follower.refresh(&done, true, 16.5);
    assert!(!later.bubble && later.row == 0, "{later:?}");
    // A click on the pet opens it again, and it stays.
    assert!(!follower.toggle(true));
    assert!(follower.refresh(&done, true, 30.0).bubble);
    // With the task setting off, the click turns it on.
    assert!(follower.toggle(false));
}

#[test]
fn a_request_card_is_keyed_by_its_tool_and_input() {
    let one = follow::card_key(Some(("run", "req", "Bash", "{\"command\":\"ls\"}")), Some("run"));
    let other = follow::card_key(Some(("run", "req", "Bash", "{\"command\":\"rm\"}")), Some("run"));
    assert!(one.starts_with("run|req|") && one.len() == "run|req|".len() + 64 && one != other);
    assert_eq!(follow::card_key(None, Some("pane")), "pane");
    assert_eq!(follow::card_key(None, None), "idle");
}
