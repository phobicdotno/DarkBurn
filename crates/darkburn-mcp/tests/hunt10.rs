//! Bug hunt 10: tool arguments are checked against the whole schema the tool lists, not only
//! their top-level types.

mod common;

use common::*;
use serde_json::json;

#[test]
fn a_zero_page_size_is_refused_instead_of_paging_forever() {
    let mut s = default_server();
    // `limit` is at least 1 by the schema; 0 gave an empty page whose `next_offset` was the same
    // offset again, so a client that follows `next_offset` never finished.
    let r = call(&mut s, "list_commands", json!({"limit": 0}));
    assert!(is_error(&r), "{}", text(&r));
    assert!(text(&r).contains("invalid_params"), "{}", text(&r));
}

#[test]
fn a_misspelt_field_of_a_batch_step_is_refused_and_nothing_runs() {
    let mut s = default_server();
    // `parms` is not a field of a step (the schema closes the step object); it was dropped and
    // the step ran with no parameters at all.
    let r = call(&mut s, "batch", json!({"steps": [{"command": "file.new", "parms": {"x": 1}}]}));
    assert!(is_error(&r), "{}", text(&r));
    assert!(text(&r).contains("invalid_params"), "{}", text(&r));
    let ev = json_of(&call(&mut s, "events", json!({})));
    assert!(!ev.to_string().contains("\"file.new\""), "the step ran: {ev}");
}

#[test]
fn a_screenshot_region_of_no_size_is_refused() {
    let mut s = default_server();
    let r = call(&mut s, "screenshot", json!({"region": {"x": 0, "y": 0, "width": 0, "height": -5}}));
    assert!(is_error(&r), "{r}");
    assert!(text(&r).contains("invalid_params"), "{}", text(&r));
}

#[test]
fn a_wait_past_the_longest_is_still_shortened_not_refused() {
    let mut s = default_server();
    let r = call(&mut s, "execute", json!({"command": "file.new", "wait_ms": u64::MAX}));
    assert!(!is_error(&r), "{}", text(&r));
}
