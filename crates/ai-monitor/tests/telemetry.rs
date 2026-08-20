//! Headless assertions for the `telemetry-model` fold.
//!
//! Every input is either an inline literal or the checked-in fixture at
//! `tests/fixtures/session.jsonl` — never a live log, and no clock is read, so the
//! numbers are the same on every machine and every run (R10.1).

use ai_monitor::telemetry::{
    classify, parse_task_notification, FileOp, SessionTelemetry, TelemetrySnapshot, ToolKind,
};

fn fold(lines: &str) -> TelemetrySnapshot {
    let mut t = SessionTelemetry::new();
    for l in lines.lines() {
        t.apply(l);
    }
    t.snapshot()
}

fn fixture() -> TelemetrySnapshot {
    fold(include_str!("fixtures/session.jsonl"))
}

// ---- R0.1 the whole fixture -------------------------------------------------

#[test]
fn fixture_transcript_folds_to_expected_telemetry() {
    let s = fixture();

    // identity, first seen
    assert_eq!(s.session_id.as_deref(), Some("5e59b629-aaaa"));
    assert_eq!(s.model.as_deref(), Some("claude-opus-5"));

    // 3 distinct messages: msg_A (x4 replicas), msg_B, msg_C
    assert_eq!(s.total_output, 441 + 50 + 10, "replicas folded once");
    assert_eq!(s.total_input, 2 + 3 + 1);
    assert_eq!(s.total_cache_read, 111_577 + 120_000);
    assert_eq!(s.total_cache_creation, 3_355 + 100);

    // gauge = LATEST usage, not a sum
    // msg_C's input(1) + cache_read(0) + cache_creation(0) — the LATEST usage, not a sum
    assert_eq!(s.context_used, 1, "msg_C is the most recent usage");

    // time comes from the log
    assert_eq!(s.first_event_ms, Some(1_785_223_020_461));
    assert_eq!(s.last_event_ms, Some(1_785_223_260_000));

    // activity
    assert_eq!(s.tools.len(), 1, "only the plain Bash tool lands here");
    assert_eq!(s.tools[0].name, "Bash");
    assert!(!s.tools[0].ok, "is_error:true is a failure");

    assert_eq!(s.files.len(), 1);
    assert_eq!(s.files[0].op, FileOp::Read);
    assert_eq!(s.files[0].path, "/proj/crates/renderer/atlas.rs");
    assert!(s.files[0].ok, "absent is_error reads as success");
    assert_eq!(s.files[0].duration_ms, Some(1537));

    // both skill sources recorded, newest first
    assert_eq!(s.skills.len(), 2);
    assert_eq!(s.skills[0].name, "harness-ops:decompose", "wrapper form");
    assert_eq!(s.skills[1].name, "harness-ops:specify", "Skill tool_use");

    assert_eq!(s.subagents.len(), 1);
    assert_eq!(
        s.subagents[0].subagent_type.as_deref(),
        Some("general-purpose")
    );
    assert_eq!(s.subagents[0].model.as_deref(), Some("opus"));
    assert!(s.subagents[0].completed);

    assert_eq!(s.tasks.len(), 1);
    assert_eq!(s.tasks[0].0, "1");
    assert_eq!(s.tasks[0].1.subject, "T1: Split title model");
    assert_eq!(s.tasks[0].1.status, "in_progress");

    // diagnostics
    assert_eq!(s.orphan_tool_results, 1, "toolu_ORPHAN");
    assert_eq!(s.parse_errors, 0);
    assert_eq!(s.usage_conflicts, 0);
    assert_eq!(s.unparsed_task_creates, 0);
}

// ---- R1 tokens ---------------------------------------------------------------

#[test]
fn replicated_usage_folds_once() {
    // R1.1 — the headline defect: 4 lines, one message, one fold.
    let l = |blk: &str| {
        format!(
            r#"{{"type":"assistant","requestId":"req_A","message":{{"id":"msg_A","usage":{{"input_tokens":2,"output_tokens":441}},"content":[{{"type":"{blk}"}}]}}}}"#
        )
    };
    let s = fold(&[l("thinking"), l("text"), l("tool_use"), l("tool_use")].join("\n"));
    assert_eq!(s.total_output, 441, "not 1764");
    assert_eq!(s.total_input, 2);
}

#[test]
fn retry_under_a_new_request_id_folds_again() {
    // R1.2 — a retry is admitted even when it re-bills an identical amount: two
    // genuinely different requests both carry ids, so `unseen id on a key that
    // already had one` separates them without needing the numbers to differ.
    let s = fold(concat!(
        r#"{"type":"assistant","requestId":"req_A","message":{"id":"msg_A","usage":{"output_tokens":100}}}"#,
        "\n",
        r#"{"type":"assistant","requestId":"req_B","message":{"id":"msg_A","usage":{"output_tokens":100}}}"#
    ));
    assert_eq!(s.total_output, 200);
}

#[test]
fn missing_request_id_still_collapses_replicas() {
    // R1.3 — degrades to message.id, never to per-line counting.
    let s = fold(concat!(
        r#"{"type":"assistant","message":{"id":"msg_A","usage":{"output_tokens":50}}}"#,
        "\n",
        r#"{"type":"assistant","message":{"id":"msg_A","usage":{"output_tokens":50}}}"#
    ));
    assert_eq!(s.total_output, 50);
}

#[test]
fn differing_replica_is_refused_and_counted() {
    // R1.4 — a difference means the byte-identical assumption broke; make it visible.
    let s = fold(concat!(
        r#"{"type":"assistant","requestId":"r","message":{"id":"m","usage":{"output_tokens":10}}}"#,
        "\n",
        r#"{"type":"assistant","requestId":"r","message":{"id":"m","usage":{"output_tokens":999}}}"#
    ));
    assert_eq!(s.total_output, 10, "first wins");
    assert_eq!(s.usage_conflicts, 1);
}

#[test]
fn context_is_a_gauge_and_totals_are_sums() {
    // R1.5 / R1.6
    let s = fold(concat!(
        r#"{"type":"assistant","requestId":"1","message":{"id":"a","usage":{"input_tokens":2,"cache_read_input_tokens":100,"cache_creation_input_tokens":10}}}"#,
        "\n",
        r#"{"type":"assistant","requestId":"2","message":{"id":"b","usage":{"input_tokens":3,"cache_read_input_tokens":200,"cache_creation_input_tokens":20}}}"#
    ));
    assert_eq!(s.context_used, 3 + 200 + 20, "latest only");
    assert_eq!(s.total_cache_read, 300, "cumulative");
}

// ---- R2 time -----------------------------------------------------------------

#[test]
fn timestampless_line_does_not_move_the_clock() {
    // R2.2 — a `permission-mode` bookkeeping line must not reset idle to zero.
    let s = fold(concat!(
        r#"{"type":"assistant","timestamp":"2026-07-28T07:17:00.461Z","message":{"id":"a"}}"#,
        "\n",
        r#"{"type":"permission-mode","mode":"default"}"#
    ));
    assert_eq!(s.last_event_ms, Some(1_785_223_020_461));
}

#[test]
fn unparseable_timestamp_is_ignored_not_defaulted() {
    // R2.3
    let s = fold(r#"{"type":"assistant","timestamp":"not-a-date","message":{"id":"a"}}"#);
    assert_eq!(s.first_event_ms, None);
    assert_eq!(s.last_event_ms, None);
}

// ---- R3 tools ----------------------------------------------------------------

#[test]
fn top_level_tool_events_correlate_too() {
    // R3.2 — not just the nested message.content form.
    let s = fold(concat!(
        r#"{"type":"tool_use","id":"t1","name":"Bash","timestamp":"2026-07-28T07:17:00.000Z"}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"t1","timestamp":"2026-07-28T07:17:02.000Z"}"#
    ));
    assert_eq!(s.tools.len(), 1);
    assert_eq!(s.tools[0].duration_ms, Some(2000));
}

#[test]
fn missing_or_negative_duration_is_none_never_zero() {
    // R3.5 — zero would render as a real sub-millisecond measurement.
    let s = fold(concat!(
        r#"{"type":"tool_use","id":"t1","name":"Bash","timestamp":"2026-07-28T07:17:05.000Z"}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"t1","timestamp":"2026-07-28T07:17:00.000Z"}"#,
        "\n",
        r#"{"type":"tool_use","id":"t2","name":"Bash"}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"t2"}"#
    ));
    assert_eq!(s.tools.len(), 2);
    assert!(s.tools.iter().all(|t| t.duration_ms.is_none()));
}

#[test]
fn repeated_tool_use_id_replaces_and_counts() {
    // R3.6 — last-wins; attributing the result to a stale start would be worse.
    let s = fold(concat!(
        r#"{"type":"tool_use","id":"t1","name":"Bash","timestamp":"2026-07-28T07:17:00.000Z"}"#,
        "\n",
        r#"{"type":"tool_use","id":"t1","name":"Bash","timestamp":"2026-07-28T07:17:04.000Z"}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"t1","timestamp":"2026-07-28T07:17:05.000Z"}"#
    ));
    assert_eq!(s.duplicate_tool_ids, 1);
    assert_eq!(
        s.tools[0].duration_ms,
        Some(1000),
        "measured from the second"
    );
}

#[test]
fn explicit_is_error_false_reads_as_success() {
    // R3.3 — the three-way distinction: absent, false, and true. Only `true` fails.
    let s = fold(concat!(
        r#"{"type":"tool_use","id":"a","name":"Bash"}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"a"}"#,
        "\n",
        r#"{"type":"tool_use","id":"b","name":"Bash"}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"b","is_error":false}"#,
        "\n",
        r#"{"type":"tool_use","id":"c","name":"Bash"}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"c","is_error":true}"#
    ));
    // newest-first: c, b, a
    assert_eq!(s.tools.len(), 3);
    assert!(!s.tools[0].ok, "explicit true -> failure");
    assert!(s.tools[1].ok, "explicit false -> success");
    assert!(s.tools[2].ok, "absent -> success");
}

#[test]
fn file_tools_extract_path_or_pattern_per_operation() {
    // R3.8 — and the seam the app depends on: Grep/Glob carry a PATTERN, not a
    // path, so the app must not basename-shorten them.
    let s = fold(concat!(
        r#"{"type":"tool_use","id":"w","name":"Write","input":{"file_path":"/p/out.rs"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"w"}"#,
        "\n",
        r#"{"type":"tool_use","id":"g","name":"Grep","input":{"pattern":"src/.*/mod\\.rs"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"g"}"#,
        "\n",
        r#"{"type":"tool_use","id":"l","name":"Glob","input":{"pattern":"**/*.toml"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"l"}"#
    ));
    assert_eq!(s.files.len(), 3);
    let by = |op| s.files.iter().find(|f| f.op == op).unwrap();
    assert_eq!(by(FileOp::Write).path, "/p/out.rs");
    assert_eq!(
        by(FileOp::Grep).path,
        r"src/.*/mod\.rs",
        "pattern kept verbatim"
    );
    assert_eq!(by(FileOp::List).path, "**/*.toml");
}

#[test]
fn agents_are_a_flat_list_with_no_parent_field() {
    // R6.3 — real Claude Code JSONL carries no parent linkage (verified across 49
    // Agent calls), so the reference's tree is not reconstructable and the app must
    // render a list. Even an `input.parentId` must not create a hierarchy.
    let s = fold(concat!(
        r#"{"type":"tool_use","id":"a1","name":"Agent","input":{"subagent_type":"x"}}"#,
        "\n",
        r#"{"type":"tool_use","id":"a2","name":"Agent","input":{"subagent_type":"y","parentId":"a1"}}"#,
        "\n",
        r#"{"type":"tool_use","id":"a3","name":"Agent","input":{"subagent_type":"z"}}"#
    ));
    assert_eq!(s.subagents.len(), 3);
    assert!(s.subagents.iter().all(|a| !a.completed));
    // The type carries no parent/depth field at all — asserted structurally by the
    // fact that every entry is constructed identically regardless of parentId.
    assert_eq!(
        s.subagents[0].subagent_type.as_deref(),
        Some("z"),
        "newest first"
    );
}

#[test]
fn classification_never_falls_through_to_skill() {
    // R3.7 — the TS reference buckets unknown tools as skills; we do not.
    assert_eq!(classify("Agent"), ToolKind::Agent);
    assert_eq!(classify("Skill"), ToolKind::Skill);
    assert_eq!(classify("Read"), ToolKind::File(FileOp::Read));
    assert_eq!(classify("Grep"), ToolKind::File(FileOp::Grep));
    assert_eq!(classify("TaskCreate"), ToolKind::Task);
    assert_eq!(classify("Bash"), ToolKind::Plain);
    assert_eq!(classify("SomethingNew"), ToolKind::Plain);
}

// ---- R4 bounds ---------------------------------------------------------------

#[test]
fn rings_and_maps_are_capped() {
    // R4.1 / R4.2 — nothing grows without limit in a long-lived terminal.
    let mut t = SessionTelemetry::new();
    for i in 0..100 {
        t.apply(&format!(
            r#"{{"type":"tool_use","id":"t{i}","name":"Bash","timestamp":"2026-07-28T07:17:00.000Z"}}"#
        ));
        t.apply(&format!(r#"{{"type":"tool_result","tool_use_id":"t{i}"}}"#));
    }
    assert_eq!(t.snapshot().tools.len(), 32);

    // 100 unmatched tool_uses must not leak
    let mut t2 = SessionTelemetry::new();
    for i in 0..100 {
        t2.apply(&format!(
            r#"{{"type":"tool_use","id":"p{i}","name":"Bash"}}"#
        ));
    }
    // nothing completed, so nothing is in the ring; the pending map is bounded
    // internally — assert the observable consequence: an old id is gone.
    t2.apply(r#"{"type":"tool_result","tool_use_id":"p0"}"#);
    assert_eq!(t2.snapshot().orphan_tool_results, 1, "p0 was evicted");
}

#[test]
fn dedupe_cap_does_not_corrupt_totals_in_normal_use() {
    // R4.3 — 200 distinct messages x 4 contiguous replicas each. Replica runs are
    // adjacent, so eviction never splits one and the totals are exact.
    let mut t = SessionTelemetry::new();
    for i in 0..200 {
        for _ in 0..4 {
            t.apply(&format!(
                r#"{{"type":"assistant","requestId":"r{i}","message":{{"id":"m{i}","usage":{{"output_tokens":10}}}}}}"#
            ));
        }
    }
    assert_eq!(
        t.snapshot().total_output,
        2000,
        "200 x 10, replicas collapsed"
    );
}

#[test]
fn dedupe_set_is_genuinely_bounded() {
    // The test above would ALSO pass with DEDUPE_CAP removed, so it proves nothing
    // about the bound. This one observes the cap directly: fold 65 distinct
    // messages (cap is 64), then replay the FIRST one. Its key has been evicted, so
    // it folds a second time — D1's accepted failure mode, and the only externally
    // visible evidence that the set is bounded at all.
    let msg = |i: usize| {
        format!(
            r#"{{"type":"assistant","requestId":"r{i}","message":{{"id":"m{i}","usage":{{"output_tokens":1}}}}}}"#
        )
    };
    let mut t = SessionTelemetry::new();
    for i in 0..65 {
        t.apply(&msg(i));
    }
    assert_eq!(t.snapshot().total_output, 65);
    t.apply(&msg(0));
    assert_eq!(
        t.snapshot().total_output,
        66,
        "m0 was evicted past the 64-entry cap, so its replay re-folded"
    );
}

#[test]
fn every_ring_and_the_task_map_is_capped() {
    // R4.1 / R4.4 — Gate 2 requires driving past EACH cap, not just the tools ring.
    let mut t = SessionTelemetry::new();
    for i in 0..40 {
        // skills (cap 16) via the wrapper form
        t.apply(&format!(
            r#"{{"type":"user","message":{{"role":"user","content":"<command-name>/s{i}</command-name>"}}}}"#
        ));
        // files (cap 32)
        t.apply(&format!(
            r#"{{"type":"tool_use","id":"f{i}","name":"Read","input":{{"file_path":"/p/{i}.rs"}}}}"#
        ));
        t.apply(&format!(r#"{{"type":"tool_result","tool_use_id":"f{i}"}}"#));
        // subagents (cap 32)
        t.apply(&format!(
            r#"{{"type":"tool_use","id":"a{i}","name":"Agent","input":{{"subagent_type":"g"}}}}"#
        ));
    }
    let s = t.snapshot();
    assert_eq!(s.skills.len(), 16, "skills ring");
    assert_eq!(s.files.len(), 32, "file-activity ring");
    assert_eq!(s.subagents.len(), 32, "subagents ring");

    // tasks map (cap 64), evicting the LEAST-RECENTLY-UPDATED
    let mut t2 = SessionTelemetry::new();
    let create = |i: usize, ms: &str| {
        vec![
            format!(
                r#"{{"type":"tool_use","id":"c{i}","name":"TaskCreate","input":{{"subject":"s{i}"}},"timestamp":"{ms}"}}"#
            ),
            format!(
                r#"{{"type":"tool_result","tool_use_id":"c{i}","content":"Task #{i} created successfully: s{i}","timestamp":"{ms}"}}"#
            ),
        ]
    };
    for i in 1..=64 {
        for l in create(i, "2026-07-28T07:00:00.000Z") {
            t2.apply(&l);
        }
    }
    // touch task #1 so it is the most-recently-updated, BEFORE the overflow create
    t2.apply(r#"{"type":"tool_use","id":"u1","name":"TaskUpdate","input":{"taskId":"1","status":"in_progress"},"timestamp":"2026-07-28T09:00:00.000Z"}"#);
    t2.apply(r#"{"type":"tool_result","tool_use_id":"u1","timestamp":"2026-07-28T09:00:00.000Z"}"#);
    for l in create(65, "2026-07-28T08:00:00.000Z") {
        t2.apply(&l);
    }
    let s2 = t2.snapshot();
    assert_eq!(s2.tasks.len(), 64, "tasks map capped");
    assert!(
        s2.tasks.iter().any(|(id, _)| id == "1"),
        "task 1 was the most-recently-updated, so LRU must retain it"
    );
}

#[test]
fn no_input_can_panic_the_fold() {
    // Constraint: "Never panic on any input, well-formed or not." Unchecked `+=` on
    // the token totals overflowed on a hostile u64::MAX; every path now saturates.
    let huge = r#"{"type":"assistant","requestId":"r","message":{"id":"m","usage":{"input_tokens":18446744073709551615,"output_tokens":18446744073709551615,"cache_read_input_tokens":18446744073709551615,"cache_creation_input_tokens":18446744073709551615}}}"#;
    let mut t = SessionTelemetry::new();
    t.apply(huge);
    t.apply(
        &huge
            .replace(r#""id":"m""#, r#""id":"m2""#)
            .replace(r#""r""#, r#""r2""#),
    );
    let s = t.snapshot();
    assert_eq!(
        s.total_output,
        u64::MAX,
        "saturated, not wrapped, not panicked"
    );
    assert_eq!(s.context_used, u64::MAX);
}

#[test]
fn a_zero_context_usage_replaces_the_gauge() {
    // D2 — the gauge is LATEST-value. A `> 0` guard would leave a stale reading
    // standing when the newest usage genuinely reports zero context.
    let s = fold(concat!(
        r#"{"type":"assistant","requestId":"1","message":{"id":"a","usage":{"input_tokens":500}}}"#,
        "
",
        r#"{"type":"assistant","requestId":"2","message":{"id":"b","usage":{"output_tokens":7}}}"#
    ));
    assert_eq!(s.context_used, 0, "newest usage has no context fields");
    assert_eq!(s.total_input, 500, "the cumulative total still holds it");
}

#[test]
fn replicas_disagreeing_about_request_id_still_collapse() {
    // Keying on a concatenated "id|requestId" split this case into two keys and
    // double-counted. The id is the identity; requestId only discriminates retries.
    let s = fold(concat!(
        r#"{"type":"assistant","requestId":"r","message":{"id":"m","usage":{"output_tokens":100}}}"#,
        "
",
        r#"{"type":"assistant","message":{"id":"m","usage":{"output_tokens":100}}}"#
    ));
    assert_eq!(s.total_output, 100);
}

// ---- R5 / R6 / R7 skills, agents, tasks --------------------------------------

#[test]
fn attribution_skill_never_creates_an_entry() {
    // R5.3 — it is ambient on every line of a skill's execution (41 lines for one
    // /decompose in real data), so keying on it would yield 41 entries.
    let mut t = SessionTelemetry::new();
    for _ in 0..41 {
        t.apply(r#"{"type":"assistant","attributionSkill":"harness-ops:decompose","message":{"id":"a"}}"#);
    }
    assert!(t.snapshot().skills.is_empty());
}

#[test]
fn command_name_wrapper_detected_in_both_content_forms() {
    // R5.2 — the array-text form is the one the TS reference would miss.
    let s = fold(concat!(
        r#"{"type":"user","message":{"role":"user","content":"<command-name>/foo</command-name>"}}"#,
        "\n",
        r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<command-name>/bar</command-name>"}]}}"#
    ));
    assert_eq!(s.skills.len(), 2);
    assert_eq!(s.skills[0].name, "bar");
    assert_eq!(s.skills[1].name, "foo");
}

#[test]
fn skill_entries_carry_only_a_start_time() {
    // R5.4 — the tool_use -> tool_result pair is ~50ms of instruction-load latency,
    // NOT execution, so it must never surface as a duration. The fixture's Skill
    // pair spans exactly 50ms; assert the start is the tool_use instant and that
    // nothing anywhere in the entry equals that 50ms interval.
    let s = fixture();
    let specify = s
        .skills
        .iter()
        .find(|k| k.name == "harness-ops:specify")
        .unwrap();
    assert_eq!(
        specify.start_ms,
        Some(1_785_223_080_000),
        "the tool_use instant"
    );
    // The completed-tools ring is where a duration WOULD have been recorded had the
    // skill been classified as a plain tool; prove it is not there either.
    assert!(!s.tools.iter().any(|t| t.name == "Skill"));
}

#[test]
fn an_in_flight_agent_is_visible_before_it_completes() {
    // R6.1 — a not-completed entry enters the ring at tool_use.
    let s = fold(
        r#"{"type":"tool_use","id":"a1","name":"Agent","input":{"subagent_type":"general-purpose"}}"#,
    );
    assert_eq!(s.subagents.len(), 1);
    assert!(!s.subagents[0].completed);
    assert!(s.subagents[0].end_ms.is_none());

    // agent-elapsed-sidechain R6.1/R7.1 — the two lifecycle fields open honest.
    // `id` is the tool_use_id; the agent's own id is not knowable until its result.
    assert_eq!(s.subagents[0].id, "a1");
    assert_eq!(s.subagents[0].agent_id, None);
    // `true`, not `false`: this value survives on every agent the notification branch
    // never touches — in-flight, synchronously completed, and never-notified alike.
    // Seeding it `false` would render all of them as failures.
    assert!(s.subagents[0].ended_ok);
}

// ---- agent-elapsed-sidechain R1/R2/R3: launch vs completion -------------------

/// An `Agent` tool_use, then a result of the caller's choosing.
fn agent_pair(result: &str) -> TelemetrySnapshot {
    fold(&format!(
        "{}\n{}",
        r#"{"type":"tool_use","id":"a1","name":"Agent","input":{"subagent_type":"general-purpose"},"timestamp":"2026-08-19T00:00:00.000Z"}"#,
        result
    ))
}

#[test]
fn an_async_launch_does_not_complete_the_agent() {
    // R1.1 / R2.1 / R3.1 — the key witness fires, so the receipt is not an ending.
    let s = agent_pair(
        r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:00:00.083Z","content":"(receipt)","toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"agent-11"}}"#,
    );
    assert_eq!(s.subagents.len(), 1);
    let a = &s.subagents[0];
    assert!(!a.completed, "a launch is a receipt, not an answer");
    assert_eq!(a.end_ms, None);
    // The start is untouched, so the card measures from the real launch.
    assert_eq!(a.start_ms, Some(1_787_097_600_000));
    assert_eq!(a.agent_id.as_deref(), Some("agent-11"));
    // The two ids are different keys and neither replaces the other: `id` stays the
    // tool_use_id of the call, `agent_id` is the agent's own. Confusing them is how the
    // first draft of this feature joined on the wrong thing.
    assert_eq!(a.id, "a1", "the tool_use_id is not overwritten");
}

#[test]
fn the_launch_prose_alone_marks_a_launch() {
    // R1.2 — the second witness stands on its own, on a line with no `toolUseResult`
    // at all. It exists so that dropping `isAsync` upstream cannot silently restore
    // the defect; R10.3 is the reason this is a separate test from the one above.
    let s = agent_pair(
        r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:00:00.083Z","content":"Async agent launched successfully. It will report when it finishes."}"#,
    );
    assert!(!s.subagents[0].completed);
    assert_eq!(s.subagents[0].end_ms, None);
}

#[test]
fn the_launch_prose_on_a_bash_result_touches_no_agent() {
    // R1.3 — the witnesses are consulted ONLY inside the Agent arm. Grepping for the
    // prose echoes it back into the log as a Bash result; if that classified anything,
    // a real in-flight agent would be stranded as a permanent LIVE ghost.
    let s = fold(&[
        r#"{"type":"tool_use","id":"a1","name":"Agent","input":{"subagent_type":"Explore"},"timestamp":"2026-08-19T00:00:00.000Z"}"#,
        r#"{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"grep -r 'Async agent launched successfully.' ."},"timestamp":"2026-08-19T00:00:01.000Z"}"#,
        r#"{"type":"tool_result","tool_use_id":"b1","timestamp":"2026-08-19T00:00:02.000Z","content":"Async agent launched successfully."}"#,
    ]
    .join("\n"));
    assert_eq!(s.subagents.len(), 1, "Bash never enters the agent ring");
    assert!(!s.subagents[0].completed, "still in flight, untouched");
    assert_eq!(s.subagents[0].agent_id, None);
    assert_eq!(s.total_tool_calls, 1, "the Bash call is tallied normally");
    // "As an ordinary completed tool" means it lands in the completed-tools ring, which
    // is the part a bare counter assertion would not have caught.
    assert!(
        s.tools.iter().any(|t| t.name == "Bash" && t.ok),
        "the Bash call completed like any other: {:?}",
        s.tools
    );
}

#[test]
fn an_unrecognised_agent_result_still_completes() {
    // R1.4 — neither witness fires, so the pre-existing behaviour stands. The
    // conservative direction: an upstream format change degrades the panel to today's
    // defect, not to one where nothing ever completes.
    let s = agent_pair(
        r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:00:30.000Z","content":"Found 3 issues.","toolUseResult":{"someShapeNobodyHasSeen":true}}"#,
    );
    assert!(s.subagents[0].completed);
    assert_eq!(s.subagents[0].end_ms, Some(1_787_097_630_000));
}

#[test]
fn a_synchronous_completion_completes_and_yields_its_id() {
    // R1.5 / R3.2 — absence of `isAsync` is a COMPLETION. `isAsync: false` occurs in no
    // observed log, so a `== false` test would read every sync completion as a launch.
    // The id is captured here too: this agent can still be resumed in the background
    // later, and that notification has nothing to join against without it.
    let s = agent_pair(
        r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:04:18.000Z","content":"Done.","toolUseResult":{"status":"completed","agentId":"agent-sync"}}"#,
    );
    assert!(s.subagents[0].completed);
    assert_eq!(s.subagents[0].end_ms, Some(1_787_097_858_000));
    assert_eq!(s.subagents[0].agent_id.as_deref(), Some("agent-sync"));
}

#[test]
fn the_launch_witness_is_read_from_the_line_not_the_block() {
    // R1.6 — the shape real logs use: the tool_result is nested in message.content
    // while `toolUseResult` is a sibling of `message`. The block's own text is an
    // ordinary answer, so a reader that looked only at the block would complete this
    // agent. This is the sub-requirement that forces the signature change.
    let s = agent_pair(
        r#"{"type":"user","timestamp":"2026-08-19T00:00:00.083Z","message":{"content":[{"type":"tool_result","tool_use_id":"a1","content":"(receipt)"}]},"toolUseResult":{"isAsync":true,"agentId":"agent-nested"}}"#,
    );
    assert!(
        !s.subagents[0].completed,
        "read from the line, not the block"
    );
    assert_eq!(s.subagents[0].agent_id.as_deref(), Some("agent-nested"));
}

#[test]
fn an_async_launch_is_still_counted_as_a_call() {
    // R2.2 — `tally_call` fires before the per-kind branch and is not moved. The
    // launch is a delivered tool_result and stays one for accounting; only the ring's
    // reading of it changed.
    let s = agent_pair(
        r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:00:00.083Z","content":"(receipt)","toolUseResult":{"isAsync":true,"agentId":"agent-11"}}"#,
    );
    assert_eq!(s.total_tool_calls, 1);
    assert_eq!(s.failed_tool_calls, 0);
    // The by-name map too, not just the totals — "counted exactly as before" is a claim
    // about every counter `tally_call` maintains.
    assert_eq!(
        s.tool_calls
            .iter()
            .find(|(n, _)| n == "Agent")
            .map(|(_, c)| *c),
        Some(1),
        "the Agent call is counted under its own name: {:?}",
        s.tool_calls
    );
}

// ---- agent-elapsed-sidechain R4: recognising the notification -----------------

/// A notification in the field order upstream emits, with a caller-chosen body.
fn notification(task_id: &str, status: &str, result: &str) -> String {
    format!(
        "<task-notification>\n\
         <task-id>{task_id}</task-id>\n\
         <tool-use-id>toolu_the_call_that_woke_it</tool-use-id>\n\
         <output-file>/tmp/agent.jsonl</output-file>\n\
         <status>{status}</status>\n\
         <summary>swept the crate</summary>\n\
         <note>none</note>\n\
         <result>{result}</result>\n\
         <usage><subagent_tokens>1200</subagent_tokens></usage>\n\
         </task-notification>"
    )
}

/// A `user` record whose `message.content` is a bare string — how a real
/// notification arrives.
fn bare_string_line(content: &str, ts: &str) -> String {
    serde_json::json!({
        "type": "user",
        "timestamp": ts,
        "message": {"content": content},
    })
    .to_string()
}

/// The same text carried as an array `text` block — how prose ABOUT a notification
/// arrives.
fn text_block_line(text: &str, ts: &str) -> String {
    serde_json::json!({
        "type": "user",
        "timestamp": ts,
        "message": {"content": [{"type": "text", "text": text}]},
    })
    .to_string()
}

#[test]
fn a_notification_that_starts_with_the_tag_parses() {
    // R4.1 — the join key and the status come out; the tool-use-id is read by nothing.
    let n = parse_task_notification(&notification("agent-11", "completed", "swept 4 files"))
        .expect("a genuine notification parses");
    assert_eq!(n.task_id, "agent-11");
    assert_eq!(n.status.as_deref(), Some("completed"));
}

#[test]
fn a_summary_that_quotes_a_notification_does_not_parse() {
    // R4.2 — the live counterexample: a `/compact` continuation summary recaps a
    // notification inside its own prose. Under a `contains` gate this ends a running
    // agent at the summary's timestamp; under starts-with it is inert.
    let summary = format!(
        "This session is being continued from a previous conversation that ran out of \
         context. The summary below covers the earlier portion.\n\nEarlier, this arrived:\n\n{}",
        notification("agent-11", "completed", "swept 4 files")
    );
    assert!(summary.contains("<task-notification>"), "it does quote one");
    assert_eq!(parse_task_notification(&summary), None);

    // The Then is about the RING, not the parser: fold a real agent and prove the
    // summary leaves it alone. Asserting only the `None` above would pass even if the
    // branch were wired to something that ignores its own parse result.
    let s = fold(&format!(
        "{}\n{}",
        async_launch("a1", "agent-11"),
        bare_string_line(&summary, "2026-08-19T00:24:21.000Z")
    ));
    assert!(!s.subagents[0].completed, "a summary is not an ending");
    assert_eq!(s.subagents[0].end_ms, None);
}

#[test]
fn the_first_task_id_wins() {
    // R4.5 — the free-text `<result>` is the one field that can quote an inner block,
    // and it sits after every field this parse reads. First-occurrence is what keeps a
    // quoted id from hijacking the join.
    let inner = notification("agent-INNER", "failed", "nested");
    let outer = notification("agent-OUTER", "completed", &inner);
    let n = parse_task_notification(&outer).expect("parses");
    assert_eq!(n.task_id, "agent-OUTER");
    assert_eq!(n.status.as_deref(), Some("completed"));

    // Both agents in the ring, so "the outer one is the one completed" is a claim the
    // fold can actually be wrong about.
    let s = fold(&format!(
        "{}\n{}\n{}",
        async_launch("a1", "agent-OUTER"),
        async_launch("a2", "agent-INNER"),
        bare_string_line(&outer, "2026-08-19T00:24:21.000Z")
    ));
    let by = |id: &str| {
        s.subagents
            .iter()
            .find(|a| a.agent_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("no entry for {id}"))
    };
    assert!(by("agent-OUTER").completed, "the outer id joined");
    assert!(!by("agent-INNER").completed, "the quoted id did not");
}

#[test]
fn a_matched_notification_does_not_register_a_phantom_skill() {
    // R4.4 — the branch returns before `scan_command_name`. Without the short-circuit,
    // a command wrapper quoted in the free-text body would enter the skills ring as a
    // skill that never ran.
    let body = notification(
        "agent-11",
        "completed",
        "the agent ran <command-name>/deploy</command-name> for me",
    );
    let s = fold(&format!(
        "{}\n{}",
        async_launch("a1", "agent-11"),
        bare_string_line(&body, "2026-08-19T00:24:21.000Z")
    ));
    assert!(
        s.subagents[0].completed,
        "the notification still did its job"
    );
    assert!(
        !s.skills.iter().any(|k| k.name == "deploy"),
        "a notification is not a command invocation: {:?}",
        s.skills
    );
}

#[test]
fn the_notification_branch_is_reachable_only_as_a_bare_string() {
    // R4.3 — the SAME text, carried as an array `text` block, never reaches the
    // notification branch: that arm feeds arbitrary prose to `scan_command_name`, which
    // is exactly why the parse does not live there. The phantom skill appearing here
    // and NOT in the test above is the observable difference between the two arms.
    let body = notification(
        "agent-11",
        "completed",
        "the agent ran <command-name>/deploy</command-name> for me",
    );
    let s = fold(&format!(
        "{}\n{}",
        async_launch("a1", "agent-11"),
        text_block_line(&body, "2026-08-19T00:24:21.000Z")
    ));
    assert!(
        !s.subagents[0].completed,
        "the array arm never reaches the notification branch"
    );
    assert_eq!(s.subagents[0].end_ms, None);
    assert!(
        s.skills.iter().any(|k| k.name == "deploy"),
        "the array arm is untouched by this feature and still scans prose: {:?}",
        s.skills
    );
}

// ---- agent-elapsed-sidechain R5/R6/R8: joining and ending ---------------------

const T0: u64 = 1_787_097_600_000; // 2026-08-19T00:00:00.000Z

/// An `Agent` launched at T0 and dispatched to the background 83ms later.
fn async_launch(id: &str, agent_id: &str) -> String {
    [
        format!(
            r#"{{"type":"tool_use","id":"{id}","name":"Agent","input":{{"subagent_type":"general-purpose"}},"timestamp":"2026-08-19T00:00:00.000Z"}}"#
        ),
        format!(
            r#"{{"type":"tool_result","tool_use_id":"{id}","timestamp":"2026-08-19T00:00:00.083Z","content":"(receipt)","toolUseResult":{{"isAsync":true,"agentId":"{agent_id}"}}}}"#
        ),
    ]
    .join("\n")
}

#[test]
fn a_notification_ends_the_agent_it_names() {
    // R5.1 / R0.1 — the round trip. The ground truth this reproduces is a real agent
    // that ran 24m21s while its card read 0s.
    let s = fold(&format!(
        "{}\n{}",
        async_launch("a1", "agent-11"),
        bare_string_line(
            &notification("agent-11", "completed", "swept 4 files"),
            "2026-08-19T00:24:21.000Z"
        )
    ));
    let a = &s.subagents[0];
    assert!(a.completed);
    assert_eq!(a.start_ms, Some(T0));
    assert_eq!(a.end_ms, Some(T0 + 1_461_000));
    assert_eq!(
        a.end_ms.unwrap() - a.start_ms.unwrap(),
        1_461_000,
        "24m21s, measured from the launch and not from the receipt"
    );
}

#[test]
fn the_notification_tool_use_id_is_not_the_join_key() {
    // R5.2 — the majority shape: most notifications are woken by a `SendMessage`, so
    // their tool-use-id names no `Agent` call at all. Keying on it would strand them.
    let body = notification("agent-11", "completed", "done").replace(
        "toolu_the_call_that_woke_it",
        "toolu_an_unrelated_send_message",
    );
    let s = fold(&format!(
        "{}\n{}",
        async_launch("a1", "agent-11"),
        bare_string_line(&body, "2026-08-19T00:24:21.000Z")
    ));
    assert!(s.subagents[0].completed, "joined on task-id regardless");
    assert_eq!(s.subagents[0].end_ms, Some(T0 + 1_461_000));
}

#[test]
fn a_renotified_agent_accumulates_from_its_original_start() {
    // R5.3 — re-notification is real, not hypothetical: 16 distinct task-ids notified
    // more than once in the corpus as measured 2026-08-19 (a rolling ~30-day window, so
    // treat the number as an as-of reading). The card promises time ALIVE, so the second
    // notification must extend the span, not restart it.
    let s = fold(&format!(
        "{}\n{}\n{}",
        async_launch("a1", "agent-11"),
        bare_string_line(
            &notification("agent-11", "completed", "leg one"),
            "2026-08-19T00:12:00.000Z"
        ),
        bare_string_line(
            &notification("agent-11", "completed", "leg two"),
            "2026-08-19T00:16:00.000Z"
        )
    ));
    let a = &s.subagents[0];
    assert_eq!(a.start_ms, Some(T0), "one agent, one start");
    assert_eq!(a.end_ms, Some(T0 + 960_000), "last-wins: 16m, not 4m");
}

#[test]
fn a_notification_corrects_a_fallback_completion() {
    // R5.4 — the drift path. Both witnesses miss, the fallback ends the agent at 83ms,
    // and the notification then rewrites it. The card jumping 0s -> 24m21s is the
    // later, better signal winning — the alternative freezes the defect permanently.
    let s = fold(&format!(
        "{}\n{}\n{}",
        r#"{"type":"tool_use","id":"a1","name":"Agent","input":{"subagent_type":"general-purpose"},"timestamp":"2026-08-19T00:00:00.000Z"}"#,
        r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:00:00.083Z","content":"(an unrecognised receipt)","toolUseResult":{"agentId":"agent-11"}}"#,
        bare_string_line(
            &notification("agent-11", "completed", "really done"),
            "2026-08-19T00:24:21.000Z"
        )
    ));
    let a = &s.subagents[0];
    assert!(a.completed);
    assert_eq!(
        a.end_ms,
        Some(T0 + 1_461_000),
        "corrected, not left at 83ms"
    );
}

#[test]
fn a_synchronous_agent_resumed_in_the_background_accumulates() {
    // R5.5 — the reason the id is captured from EVERY result. This agent finished
    // synchronously in 4s, was resumed hours later, and its card's elapsed grows after
    // the fact. Surprising in the UI, and the source of most of this feature's reach.
    let s = fold(&format!(
        "{}\n{}\n{}",
        r#"{"type":"tool_use","id":"a1","name":"Agent","input":{"subagent_type":"Explore"},"timestamp":"2026-08-19T00:00:00.000Z"}"#,
        r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:00:04.000Z","content":"Mapped the seam.","toolUseResult":{"status":"completed","agentId":"agent-sync"}}"#,
        bare_string_line(
            &notification("agent-sync", "completed", "resumed and finished"),
            "2026-08-19T02:13:00.000Z"
        )
    ));
    let a = &s.subagents[0];
    assert_eq!(a.end_ms, Some(T0 + 7_980_000), "4s becomes 2h13m");
    assert_eq!(a.start_ms, Some(T0));
}

#[test]
fn a_notification_is_not_a_tool_call() {
    // R5.6 — the notification mutates the ring and nothing else. `tally_call` stays the
    // single point a call is counted, and it is not reached from this path.
    let with = fold(&format!(
        "{}\n{}",
        async_launch("a1", "agent-11"),
        bare_string_line(
            &notification("agent-11", "completed", "done"),
            "2026-08-19T00:24:21.000Z"
        )
    ));
    let without = fold(&async_launch("a1", "agent-11"));
    assert_eq!(with.total_tool_calls, without.total_tool_calls);
    assert_eq!(with.failed_tool_calls, without.failed_tool_calls);
    assert_eq!(with.tool_calls, without.tool_calls);
}

#[test]
fn a_notification_for_an_unknown_agent_changes_nothing() {
    // R6.2 — a background `Bash` task notifies with the same block but is not a
    // subagent and has no card. Nothing is created, nothing is counted.
    let s = fold(&format!(
        "{}\n{}",
        async_launch("a1", "agent-11"),
        bare_string_line(
            &notification("agent-NOBODY", "completed", "a background Bash task"),
            "2026-08-19T00:24:21.000Z"
        )
    ));
    assert_eq!(s.subagents.len(), 1, "no phantom entry");
    assert!(!s.subagents[0].completed, "the real agent is untouched");
    assert_eq!(s.subagents[0].end_ms, None);
    // "Uncounted" is half the requirement: an unmatched notification must not move a
    // counter either, which is why no field was added to carry a tally of them.
    let baseline = fold(&async_launch("a1", "agent-11"));
    assert_eq!(s.total_tool_calls, baseline.total_tool_calls);
    assert_eq!(s.failed_tool_calls, baseline.failed_tool_calls);
    assert_eq!(s.orphan_tool_results, baseline.orphan_tool_results);
    assert_eq!(s.tool_calls, baseline.tool_calls);
}

#[test]
fn an_agent_aged_out_of_the_ring_is_not_resurrected() {
    // R6.3 — the ring's contract is "the latest 32". Re-inserting an evicted agent
    // would displace a newer one and reorder the list under the user.
    let mut lines: Vec<String> = (0..33)
        .map(|i| async_launch(&format!("a{i}"), &format!("agent-{i}")))
        .collect();
    lines.push(bare_string_line(
        &notification("agent-0", "completed", "long gone"),
        "2026-08-19T00:24:21.000Z",
    ));
    let s = fold(&lines.join("\n"));
    assert_eq!(s.subagents.len(), 32, "the cap holds");
    assert!(
        !s.subagents
            .iter()
            .any(|a| a.agent_id.as_deref() == Some("agent-0")),
        "the evicted agent stays evicted"
    );
    assert_eq!(
        s.subagents[0].agent_id.as_deref(),
        Some("agent-32"),
        "newest first, undisturbed"
    );
}

#[test]
fn a_resumed_agent_reads_done_until_its_next_notification() {
    // R8.1 — the declared gap, asserted so it is a recorded choice rather than an
    // undiscovered bug. The resume is present in the transcript below as a real
    // `SendMessage` naming the agent: without it this test would only restate R5.1 and
    // could not tell "the fold ignores SendMessage" apart from "none was there".
    // Detecting the resume would mean parsing that tool and matching `to:` against an
    // agent id; the next notification corrects the window anyway (16 task-ids were
    // measured to notify more than once — 2026-08-19, a rolling ~30-day window).
    let s = fold(&format!(
        "{}\n{}\n{}\n{}",
        async_launch("a1", "agent-11"),
        bare_string_line(
            &notification("agent-11", "completed", "leg one"),
            "2026-08-19T00:12:00.000Z"
        ),
        r#"{"type":"tool_use","id":"s1","name":"SendMessage","input":{"to":"agent-11","message":"keep going"},"timestamp":"2026-08-19T00:13:00.000Z"}"#,
        r#"{"type":"tool_result","tool_use_id":"s1","timestamp":"2026-08-19T00:13:00.200Z","content":"delivered"}"#
    ));
    assert!(
        s.subagents[0].completed,
        "the agent is running again, and the card still reads DONE"
    );
    assert_eq!(
        s.subagents[0].end_ms,
        Some(T0 + 720_000),
        "frozen at the last notification, not moved by the resume"
    );
}

#[test]
fn no_fixture_encodes_a_shape_that_never_occurs() {
    // R10.2 — `isAsync` is present-and-true or absent; no observed log carries it as
    // JSON `false`. A fixture written that way would encode a shape reality never
    // produces, and would quietly make a `== false` reading look correct.
    //
    // The forbidden literal is assembled below rather than written out, here or in any
    // comment: the Gate-1 grep guarding R10.2 scans `crates/` for that exact text, so
    // spelling it out trips the very check it documents. It did, once — which is why
    // this note exists and why the wording stays careful.
    //
    // This test owns the fixture files; the repo-wide sweep is the Gate-1 command. The
    // two together cover R10.2, and neither covers it alone.
    //
    // Reads only `tests/fixtures/`, located at compile time, so this stays machine-
    // independent and needs no `~/.claude` (R10.1).
    let key = format!("{}isAsync{}", '"', '"');
    let forbidden = [format!("{key}: false"), format!("{key}:false")];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("fixtures dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "jsonl") {
            let body = std::fs::read_to_string(&path).expect("fixture is utf-8");
            for bad in &forbidden {
                assert!(
                    !body.contains(bad.as_str()),
                    "{} encodes a shape that occurs in no log",
                    path.display()
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "no fixtures found under {}", dir.display());
}

#[test]
fn a_timestampless_notification_never_erases_a_real_end() {
    // Not in the spec's GWTs — found by the Gate-3 checker and fixed by decision.
    // `end_ms` is last-wins over VALUES; the absence of a reading is not a value. A
    // notification whose timestamp is missing or unparseable would otherwise leave
    // `completed: true, end_ms: None`, which renders as a blank card value — worse than
    // the `0s` this feature removes — and would wipe an end that was already correct.
    let launch_and_real_end = format!(
        "{}\n{}",
        async_launch("a1", "agent-11"),
        bare_string_line(
            &notification("agent-11", "completed", "the real ending"),
            "2026-08-19T00:24:21.000Z"
        )
    );

    for (label, bad_ts) in [("absent", None), ("unparseable", Some("not-a-timestamp"))] {
        let note = notification("agent-11", "completed", "a second stop, unstamped");
        let line = match bad_ts {
            Some(ts) => bare_string_line(&note, ts),
            None => serde_json::json!({"type":"user","message":{"content": note}}).to_string(),
        };
        let s = fold(&format!("{launch_and_real_end}\n{line}"));
        let a = &s.subagents[0];
        assert!(a.completed, "{label}: the log still said it stopped");
        assert_eq!(
            a.end_ms,
            Some(T0 + 1_461_000),
            "{label}: the real end survives"
        );
    }

    // And with no prior end at all, the entry completes without inventing one.
    let s = fold(&format!(
        "{}\n{}",
        async_launch("a1", "agent-11"),
        serde_json::json!({
            "type": "user",
            "message": {"content": notification("agent-11", "failed", "no stamp ever")},
        })
    ));
    assert!(s.subagents[0].completed, "never left un-completable");
    assert!(!s.subagents[0].ended_ok, "the status is still read");
    assert_eq!(s.subagents[0].end_ms, None);
}

// ---- agent-elapsed-sidechain R7: how the end went ----------------------------

/// An async launch followed by one notification carrying `status`.
fn launch_then_status(status: &str, ts: &str) -> TelemetrySnapshot {
    fold(&format!(
        "{}\n{}",
        async_launch("a1", "agent-11"),
        bare_string_line(&notification("agent-11", status, "body"), ts)
    ))
}

#[test]
fn a_completed_notification_keeps_the_end_ok() {
    // R7.2
    let s = launch_then_status("completed", "2026-08-19T00:24:21.000Z");
    assert!(s.subagents[0].completed);
    assert!(s.subagents[0].ended_ok);
}

#[test]
fn failed_and_killed_both_mark_the_end_not_ok() {
    // R7.3 — the only two non-`completed` values any log has shown. Both keep their
    // real end time: a failed run's duration is as true as a successful one's.
    for status in ["failed", "killed"] {
        let s = launch_then_status(status, "2026-08-19T00:24:21.000Z");
        assert!(s.subagents[0].completed, "{status} is still an ending");
        assert!(!s.subagents[0].ended_ok, "{status}");
        assert_eq!(s.subagents[0].end_ms, Some(T0 + 1_461_000), "{status}");
    }
}

#[test]
fn a_status_nobody_has_seen_lands_on_the_safe_side() {
    // R7.3 — a fourth value invented upstream is not `completed`, so it reads as a
    // failure rather than silently passing as success.
    let s = launch_then_status("evicted", "2026-08-19T00:24:21.000Z");
    assert!(!s.subagents[0].ended_ok);
}

#[test]
fn a_failed_agent_resumed_to_success_reads_as_success() {
    // R7.6 — `ended_ok` is last-wins, like `end_ms`. Stated and tested because it
    // follows from two rules rather than from one, which is how it would be missed.
    let s = fold(&format!(
        "{}\n{}\n{}",
        async_launch("a1", "agent-11"),
        bare_string_line(
            &notification("agent-11", "failed", "leg one blew up"),
            "2026-08-19T00:12:00.000Z"
        ),
        bare_string_line(
            &notification("agent-11", "completed", "leg two recovered"),
            "2026-08-19T00:16:00.000Z"
        )
    ));
    assert!(s.subagents[0].ended_ok, "the later signal wins");
    assert_eq!(s.subagents[0].end_ms, Some(T0 + 960_000));
}

#[test]
fn an_agent_no_notification_describes_keeps_ended_ok() {
    // R7.1 at the far end: the field is written ONLY in the notification branch, so a
    // synchronous completion and a never-notified launch both keep `true`. Seeding
    // `false` would paint every one of them as a failure.
    let sync = fold(&format!(
        "{}\n{}",
        r#"{"type":"tool_use","id":"a1","name":"Agent","input":{"subagent_type":"Explore"},"timestamp":"2026-08-19T00:00:00.000Z"}"#,
        r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:00:04.000Z","content":"Done.","toolUseResult":{"agentId":"agent-sync"}}"#
    ));
    assert!(sync.subagents[0].completed);
    assert!(sync.subagents[0].ended_ok);

    let ghost = fold(&async_launch("a1", "agent-11"));
    assert!(!ghost.subagents[0].completed);
    assert!(ghost.subagents[0].ended_ok);
}

#[test]
fn an_agent_result_without_an_agentid_leaves_it_unset() {
    // R3.3 — no id means no join key, and the entry says so rather than guessing.
    let s = agent_pair(
        r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:00:30.000Z","content":"Done.","toolUseResult":{"status":"completed"}}"#,
    );
    assert!(s.subagents[0].completed);
    assert_eq!(s.subagents[0].agent_id, None);

    // And the second half of the requirement: with no id, no notification reaches it.
    // The notification below carries a REAL task-id on purpose. An empty one would be
    // rejected by the parser before the join ever ran, and this assertion would then
    // hold no matter what the join did — passing for a reason unrelated to what it
    // claims to test.
    let with_note = fold(&format!(
        "{}\n{}\n{}",
        r#"{"type":"tool_use","id":"a1","name":"Agent","input":{"subagent_type":"Explore"},"timestamp":"2026-08-19T00:00:00.000Z"}"#,
        r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:00:00.083Z","content":"(receipt)","toolUseResult":{"isAsync":true}}"#,
        bare_string_line(
            &notification(
                "agent-11",
                "completed",
                "a real id, and no entry carries it"
            ),
            "2026-08-19T00:24:21.000Z"
        )
    ));
    assert_eq!(with_note.subagents[0].agent_id, None);
    assert!(
        !with_note.subagents[0].completed,
        "an id-less entry is unreachable, not a wildcard"
    );
    assert_eq!(with_note.subagents[0].end_ms, None);
}

#[test]
fn leading_whitespace_does_not_hide_either_gate() {
    // D1 (witness 2) and D12 both say "begins with" / "starts with" the literal. Both
    // are implemented on the TRIMMED string, which is a superset of what the spec
    // authorises. Measured logs carry no leading whitespace either way, so nothing
    // distinguishes the two on real data and the `/compact` counterexample still fails
    // both gates.
    //
    // This test pins the widening rather than hiding it: the spec does not sanction it,
    // so it is recorded here as a deliberate, visible deviation. Tightening the gates to
    // match the spec exactly should break this test, which is the point — the
    // alternative is a silent difference between what the spec says and what runs.
    let indented = format!("\n  {}", notification("agent-11", "completed", "body"));
    assert!(
        parse_task_notification(&indented).is_some(),
        "the notification gate trims first"
    );
    let s = agent_pair(
        r#"{"type":"tool_result","tool_use_id":"a1","timestamp":"2026-08-19T00:00:00.083Z","content":"\n  Async agent launched successfully. Reporting later."}"#,
    );
    assert!(
        !s.subagents[0].completed,
        "the prose witness trims first as well"
    );
}

#[test]
fn task_create_pairs_input_subject_with_result_id() {
    // R7.1 — the id exists ONLY in the result prose.
    let s = fold(concat!(
        r#"{"type":"tool_use","id":"tc","name":"TaskCreate","input":{"subject":"T1: Split title model"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"tc","content":"Task #1 created successfully: T1: Split title model"}"#
    ));
    assert_eq!(s.tasks.len(), 1);
    assert_eq!(s.tasks[0].0, "1");
    assert_eq!(s.tasks[0].1.subject, "T1: Split title model");
}

#[test]
fn failed_update_does_not_mutate() {
    // R7.3
    let s = fold(concat!(
        r#"{"type":"tool_use","id":"tc","name":"TaskCreate","input":{"subject":"S"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"tc","content":"Task #1 created successfully: S"}"#,
        "\n",
        r#"{"type":"tool_use","id":"tu","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"tu","is_error":true}"#
    ));
    assert_eq!(s.tasks[0].1.status, "active", "failed update left it alone");
}

#[test]
fn description_rewords_subject_and_add_blocked_by_is_ignored() {
    // R7.4
    let s = fold(concat!(
        r#"{"type":"tool_use","id":"tc","name":"TaskCreate","input":{"subject":"old"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"tc","content":"Task #1 created successfully: old"}"#,
        "\n",
        r#"{"type":"tool_use","id":"u1","name":"TaskUpdate","input":{"taskId":"1","description":"new"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"u1"}"#,
        "\n",
        r#"{"type":"tool_use","id":"u2","name":"TaskUpdate","input":{"taskId":"1","addBlockedBy":"2"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"u2"}"#
    ));
    assert_eq!(s.tasks[0].1.subject, "new");
    assert_eq!(s.tasks.len(), 1);
}

#[test]
fn orphan_update_and_unparseable_create_are_counted() {
    // R7.5 / R7.6
    let s = fold(concat!(
        r#"{"type":"tool_use","id":"u","name":"TaskUpdate","input":{"taskId":"7","status":"completed"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"u"}"#,
        "\n",
        r#"{"type":"tool_use","id":"c","name":"TaskCreate","input":{"tasks":[]}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"c","content":"Created 3 tasks."}"#
    ));
    assert!(s.tasks.is_empty());
    assert_eq!(s.orphan_task_updates, 1);
    assert_eq!(s.unparsed_task_creates, 1);
}

#[test]
fn task_results_never_enter_the_completed_tools_ring() {
    // R7.7 — 204 task results would otherwise evict real tool history at cap 32.
    let s = fold(concat!(
        r#"{"type":"tool_use","id":"a","name":"TaskCreate","input":{"subject":"s"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"a","content":"Task #1 created successfully: s"}"#,
        "\n",
        r#"{"type":"tool_use","id":"b","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"b"}"#,
        "\n",
        r#"{"type":"tool_use","id":"c","name":"TaskOutput","input":{"task_id":"1"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"c"}"#
    ));
    assert!(s.tools.is_empty());
}

// ---- R8 robustness -----------------------------------------------------------

#[test]
fn malformed_input_is_counted_never_fatal() {
    // R8.1
    let s = fold("not json at all\n\n{ truncated");
    assert_eq!(s.parse_errors, 2, "the empty line is not an error");
}

#[test]
fn unknown_line_type_folds_what_it_carries() {
    // R8.2
    let s = fold(r#"{"type":"file-history-delta","timestamp":"2026-07-28T07:17:00.000Z"}"#);
    assert_eq!(s.last_event_ms, Some(1_785_223_020_000));
    assert_eq!(s.parse_errors, 0);
}

#[test]
fn wrongly_typed_fields_degrade_gracefully() {
    // R8.3
    let s = fold(
        r#"{"type":"assistant","message":{"id":"a","usage":{"input_tokens":"12","output_tokens":7},"content":42}}"#,
    );
    assert_eq!(s.total_input, 0, "string is skipped");
    assert_eq!(s.total_output, 7, "readable field still folded");
}

// ---- R9 the snapshot contract ------------------------------------------------

#[test]
fn snapshot_crosses_a_thread_boundary() {
    // R9.1 — owned, Clone + Send + Sync, so the watcher can Arc it onto a channel.
    use std::sync::mpsc;
    use std::sync::Arc;

    let snap = Arc::new(fixture());
    let (tx, rx) = mpsc::channel::<Arc<TelemetrySnapshot>>();
    let sent = Arc::clone(&snap);
    std::thread::spawn(move || tx.send(sent).unwrap())
        .join()
        .unwrap();
    assert_eq!(*rx.recv().unwrap(), *snap);
}

#[test]
fn session_telemetry_is_send_and_constructible() {
    // W2a (telemetry-watcher's stated dependency on this crate).
    fn assert_send<T: Send>() {}
    assert_send::<SessionTelemetry>();
    assert_send::<TelemetrySnapshot>();
    fn assert_sync<T: Sync>() {}
    assert_sync::<TelemetrySnapshot>();
    let _ = SessionTelemetry::default();
    let _ = SessionTelemetry::new();
}

// ---- iteration-4 seams (found by the Gate-3 checker in the keying rewrite) ----

#[test]
fn a_split_retry_does_not_false_flag_a_conflict() {
    // A real retry is split across content-block lines just like the original, so
    // its own replicas must compare against the RETRY's numbers, not the first
    // request's. Otherwise `usage_conflicts` — the one instrument the spec names for
    // revealing a retry — fires on every retry it exists to admit.
    let l = |rid: &str, out: u64| {
        format!(
            r#"{{"type":"assistant","requestId":"{rid}","message":{{"id":"m","usage":{{"output_tokens":{out}}}}}}}"#
        )
    };
    let s = fold(
        &[
            l("req_A", 100),
            l("req_A", 100),
            l("req_B", 250),
            l("req_B", 250),
        ]
        .join("\n"),
    );
    assert_eq!(s.total_output, 350, "one fold per request");
    assert_eq!(s.usage_conflicts, 0, "a retry is not a conflict");
}

#[test]
fn replicas_collapse_in_either_request_id_order() {
    // The mixed-presence seam closes in BOTH directions. Present-then-absent was
    // covered; absent-then-present used to read as a retry and double-count.
    let with = r#"{"type":"assistant","requestId":"r","message":{"id":"m","usage":{"output_tokens":100}}}"#;
    let without = r#"{"type":"assistant","message":{"id":"m","usage":{"output_tokens":100}}}"#;
    assert_eq!(
        fold(&format!("{with}\n{without}")).total_output,
        100,
        "present -> absent"
    );
    assert_eq!(
        fold(&format!("{without}\n{with}")).total_output,
        100,
        "absent -> present"
    );
}

#[test]
fn the_per_message_retry_list_is_bounded() {
    // D6 is absolute: the dedupe map bounds KEYS, and this bounds each key's value.
    // Observable consequence of the cap: after 20 distinct retries the earliest
    // requestId has been evicted, so replaying it with its original (now-differing)
    // numbers reads as an unseen request and folds again. Uncapped, it would still
    // be remembered and would instead be counted a conflict.
    let l = |rid: usize, out: u64| {
        format!(
            r#"{{"type":"assistant","requestId":"r{rid}","message":{{"id":"m","usage":{{"output_tokens":{out}}}}}}}"#
        )
    };
    let mut t = SessionTelemetry::new();
    for i in 0..20 {
        t.apply(&l(i, 10 + i as u64));
    }
    let before = t.snapshot();
    assert_eq!(before.usage_conflicts, 0);
    t.apply(&l(0, 10));
    let after = t.snapshot();
    assert!(
        after.total_output > before.total_output,
        "r0 was evicted from the retry list, so it re-folded instead of being flagged"
    );
    assert_eq!(after.usage_conflicts, 0);
}

// ---- ai-monitor-right-panel R2: cumulative session counters ------------------
//
// The rings above are bounded DISPLAY windows; these are session facts. Counting a
// ring would print a number that looks cumulative and is not, which is the defect
// D3 exists to remove.

/// Fold one completed `name` call, `ok` or not. Ids are unique so no pair collides.
fn call(t: &mut SessionTelemetry, id: &mut usize, name: &str, ok: bool) {
    *id += 1;
    let n = *id;
    t.apply(&format!(
        r#"{{"type":"tool_use","id":"c{n}","name":"{name}"}}"#
    ));
    t.apply(&format!(
        r#"{{"type":"tool_result","tool_use_id":"c{n}","is_error":{}}}"#,
        !ok
    ));
}

fn count_of(s: &TelemetrySnapshot, name: &str) -> u64 {
    s.tool_calls
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, c)| *c)
        .unwrap_or(0)
}

fn tally_sum(s: &TelemetrySnapshot) -> u64 {
    s.tool_calls.iter().map(|(_, c)| *c).sum()
}

#[test]
fn the_tally_outlives_the_display_ring() {
    // R2.1 — 200 Bash calls against a 32-entry ring. The ring is what the panel can
    // list; the tally is what the session did.
    let mut t = SessionTelemetry::new();
    let mut id = 0;
    for _ in 0..200 {
        call(&mut t, &mut id, "Bash", true);
    }
    let s = t.snapshot();
    assert_eq!(s.tools.len(), 32, "TOOLS_CAP still bounds the display ring");
    assert_eq!(
        count_of(&s, "Bash"),
        200,
        "the tally is not bounded with it"
    );
    assert_eq!(s.total_tool_calls, 200);
    assert_eq!(s.failed_tool_calls, 0);
}

#[test]
fn every_kind_of_completed_call_reaches_the_tally() {
    // The tally counts CALLS, not ring entries: a Read lands in the file ring, a
    // Skill in the skills ring, an Agent in neither of those two and a TaskCreate in
    // no ring at all — but all four are calls the session made, and the design's
    // total is over calls. Folding at the single point `ok` is derived, before the
    // per-kind branch, is what makes that true without a second traversal.
    let mut t = SessionTelemetry::new();
    let mut id = 0;
    for name in ["Bash", "Read", "Skill", "Agent"] {
        call(&mut t, &mut id, name, true);
    }
    let s = t.snapshot();
    assert_eq!(s.total_tool_calls, 4);
    for name in ["Bash", "Read", "Skill", "Agent"] {
        assert_eq!(count_of(&s, name), 1, "{name} is a call");
    }
    assert_eq!(tally_sum(&s) + s.tool_calls_other, s.total_tool_calls);
}

#[test]
fn the_grand_total_is_its_own_scalar_not_the_map_sum() {
    // R2.2 — 187 calls across 70 names, i.e. past the 64-name cap, so a total
    // derived by summing the map would under-report by everything the cap merged
    // away. The total is its own counter; the identity `map + other == total` is
    // what keeps the two from drifting apart.
    let mut t = SessionTelemetry::new();
    let mut id = 0;
    for i in 0..70 {
        call(&mut t, &mut id, &format!("tool{i:02}"), true);
    }
    for _ in 0..117 {
        call(&mut t, &mut id, "tool00", true);
    }
    let s = t.snapshot();
    assert_eq!(s.total_tool_calls, 187);
    assert_eq!(
        tally_sum(&s) + s.tool_calls_other,
        187,
        "the total must be reachable from the parts, not merely plausible"
    );
    assert!(s.tool_calls.len() <= 64, "the map is still bounded");
    assert!(
        s.tool_calls_other > 0,
        "70 names past a 64 cap must have merged"
    );
    assert_eq!(
        count_of(&s, "tool00"),
        117,
        "a merged name is not banished — its later calls count again, which is the \
         difference between merging and evicting"
    );
}

#[test]
fn failed_calls_are_counted_apart_from_parser_faults() {
    // R2.3 — `parse_errors` counts PARSER faults. A tool that ran and failed is a
    // different fact, and putting one under the other's label would misreport what
    // broke. Fold both here so the two counters are seen to move independently.
    let mut t = SessionTelemetry::new();
    let mut id = 0;
    for i in 0..6 {
        call(&mut t, &mut id, "Bash", i % 2 != 0);
    }
    t.apply("not json at all");
    let s = t.snapshot();
    assert_eq!(s.failed_tool_calls, 3, "three is_error:true results");
    assert_eq!(s.total_tool_calls, 6, "a failure is still a call");
    assert_eq!(s.parse_errors, 1, "the malformed line, and nothing else");
    assert_ne!(
        s.failed_tool_calls, s.parse_errors,
        "distinct fields for distinct facts"
    );
}

#[test]
fn a_new_name_survives_the_cap_and_incumbents_are_merged() {
    // R2.4 — the anti-starvation rule (D12), stated as a test because the obvious
    // implementation is wrong. With 64 incumbents at count >= 2, "merge the smallest
    // entry" merges the ARRIVING name: it enters at 1 and is the smallest thing in
    // the map the instant it exists. Under that rule no new tool can ever be
    // recorded again, however heavily it is later used. This test fails if anyone
    // "simplifies" to evict-smallest.
    let mut t = SessionTelemetry::new();
    let mut id = 0;
    for i in 0..64 {
        for _ in 0..2 {
            call(&mut t, &mut id, &format!("inc{i:02}"), true);
        }
    }
    let before = t.snapshot();
    assert_eq!(
        before.tool_calls.len(),
        64,
        "premise: the map is exactly full"
    );
    assert_eq!(before.tool_calls_other, 0, "premise: nothing merged yet");

    call(&mut t, &mut id, "brandnew", true);
    let after = t.snapshot();

    assert_eq!(
        count_of(&after, "brandnew"),
        1,
        "the arriving name is retained — it is never a merge candidate"
    );
    assert_eq!(
        after.tool_calls.len(),
        49,
        "one batch down to the 48 watermark, then the new name — not a merge per name"
    );
    let survivors = (0..64)
        .filter(|i| count_of(&after, &format!("inc{i:02}")) == 2)
        .count();
    assert_eq!(survivors, 48, "16 incumbents merged, the rest untouched");
    assert_eq!(after.tool_calls_other, 32, "16 incumbents x 2 calls each");
    assert_eq!(after.total_tool_calls, 129);
    assert_eq!(
        tally_sum(&after) + after.tool_calls_other,
        after.total_tool_calls,
        "the sum is conserved across a merge"
    );
}

#[test]
fn the_new_counters_default_to_nothing_folded() {
    // Load-bearing beyond this crate: the app decides its empty state with
    // `*t == TelemetrySnapshot::default()` (`crates/app/src/dashboard.rs`,
    // `crates/app/src/overlay.rs`). A counter whose default meant anything other
    // than "nothing folded" would make an empty session read as an attached one.
    let fresh = SessionTelemetry::new().snapshot();
    assert_eq!(fresh, TelemetrySnapshot::default());
    let d = TelemetrySnapshot::default();
    assert!(d.tool_calls.is_empty());
    assert_eq!(d.tool_calls_other, 0);
    assert_eq!(d.total_tool_calls, 0);
    assert_eq!(d.failed_tool_calls, 0);
}

#[test]
fn the_tally_is_emitted_in_a_deterministic_name_order() {
    // The `tasks` rule, for the same reason: a HashMap has no order, and an unstable
    // snapshot would make every downstream assertion flaky. By NAME, not by count —
    // ranking is presentation, and this crate holds none (SD4).
    let mut t = SessionTelemetry::new();
    let mut id = 0;
    for (name, n) in [("zed", 1), ("alpha", 9), ("mid", 5)] {
        for _ in 0..n {
            call(&mut t, &mut id, name, true);
        }
    }
    let s = t.snapshot();
    let names: Vec<&str> = s.tool_calls.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["alpha", "mid", "zed"]);
}

// ---- ai-monitor-model-refresh: the model is the CURRENT one ------------------

/// One assistant line, in the shape real logs use.
fn assistant(id: &str, model: &str) -> String {
    format!(
        r#"{{"type":"assistant","sessionId":"64c00dbb","requestId":"{id}","message":{{"id":"{id}","model":"{model}","usage":{{"output_tokens":1}}}}}}"#
    )
}

/// One assistant line carrying usage. `cache_read + input` is what the crate sums into
/// `context_used`, which is the occupancy a real turn reports.
fn assistant_usage(id: &str, cache_read: u64, input: u64) -> String {
    format!(
        r#"{{"type":"assistant","requestId":"{id}","message":{{"id":"{id}","model":"claude-opus-5","usage":{{"input_tokens":{input},"cache_read_input_tokens":{cache_read},"output_tokens":1}}}}}}"#
    )
}

#[test]
fn a_mid_session_model_switch_reaches_the_snapshot() {
    // The real-log repro (`64c00dbb`): the session ran `claude-fable-5`, the user ran
    // `/model`, and five later assistant turns logged `claude-opus-5`. First-write-wins
    // pinned the panel to `claude-fable-5` for the rest of the session.
    let s = fold(&format!(
        "{}\n{}\n{}\n{}\n{}",
        assistant("r1", "claude-fable-5"),
        assistant("r2", "claude-fable-5"),
        // `/model` — the switch itself logs no model, so only the turns after it show it.
        r#"{"type":"user","message":{"role":"user","content":"<command-name>/model</command-name>"}}"#,
        assistant("r3", "claude-opus-5"),
        assistant("r4", "claude-opus-5"),
    ));
    assert_eq!(
        s.model.as_deref(),
        Some("claude-opus-5"),
        "the model the session is running NOW, not the one it started with"
    );
}

#[test]
fn a_trailing_synthetic_line_never_becomes_the_displayed_model() {
    // `<synthetic>` is a REAL `message.model` value — Claude Code writes it on records
    // it fabricates locally. Under an unguarded last-wins it lands at the tail of a log
    // and blanks the panel's model; it must be skipped, not folded.
    let s = fold(&format!(
        "{}\n{}",
        assistant("r1", "claude-opus-5"),
        assistant("r2", "<synthetic>"),
    ));
    assert_eq!(s.model.as_deref(), Some("claude-opus-5"));

    // ... and it does not become the model when it arrives FIRST either: skipping means
    // skipping, so the first real id still populates the field afterwards.
    let s = fold(&format!(
        "{}\n{}",
        assistant("r1", "<synthetic>"),
        assistant("r2", "claude-opus-5"),
    ));
    assert_eq!(s.model.as_deref(), Some("claude-opus-5"));

    // A log of nothing but sentinels leaves the field empty — `None` reads as "not
    // known" and the app draws a placeholder; `Some("<synthetic>")` would be a claim.
    let s = fold(&assistant("r1", "<synthetic>"));
    assert_eq!(s.model, None);

    // The rule is by SHAPE, not by the one literal — and that generality has to be
    // pinned, or it is only a comment. Without these two, `is_real_model` simplifies to
    // `m != "<synthetic>"` and every gate in the workspace still passes, silently
    // dropping both documented behaviours (found by the Gate-3 checker, which ran
    // exactly that mutation).
    let s = fold(&format!(
        "{}\n{}",
        assistant("r1", "claude-opus-5"),
        assistant("r2", "<unknown>"),
    ));
    assert_eq!(
        s.model.as_deref(),
        Some("claude-opus-5"),
        "any `<...>` sentinel is refused, not just `<synthetic>`"
    );

    let s = fold(&format!(
        "{}\n{}",
        assistant("r1", "claude-opus-5"),
        assistant("r2", ""),
    ));
    assert_eq!(
        s.model.as_deref(),
        Some("claude-opus-5"),
        "an empty id names nothing and must not overwrite a real one"
    );
}

#[test]
fn the_very_first_fold_still_populates_the_model() {
    // The half of the old behaviour that was correct. Last-wins with no prior value is
    // still a write — a guard that only ever *replaced* an existing id would leave the
    // panel blank for the whole of a single-model session.
    let s = fold(&assistant("r1", "claude-opus-5"));
    assert_eq!(s.model.as_deref(), Some("claude-opus-5"));
}

#[test]
fn the_session_id_keeps_its_first_wins_rule() {
    // Deliberately NOT symmetrical with the model above, and the asymmetry is the
    // point: a session id names the file and never changes, so the first one seen is
    // the answer. A second id in the stream is another session's line leaking in (the
    // watcher reads a tail, and rival sessions share a directory) — adopting it would
    // silently re-label the panel. Flipping both folds "for consistency" is a
    // regression, not a cleanup.
    let s = fold(
        r#"{"type":"assistant","sessionId":"first","message":{"id":"a","model":"claude-opus-5"}}
{"type":"assistant","sessionId":"second","message":{"id":"b","model":"claude-fable-5"}}"#,
    );
    assert_eq!(s.session_id.as_deref(), Some("first"), "first-wins holds");
    assert_eq!(
        s.model.as_deref(),
        Some("claude-fable-5"),
        "last-wins holds"
    );
}

#[test]
fn the_context_high_water_is_monotone_while_the_gauge_follows_occupancy_down() {
    // The gauge and the high-water answer different questions and must diverge when
    // occupancy falls. `context_used` is "how full is the window now" and legitimately
    // drops (a `/compact`); `context_high_water` is "how big has the window been proven
    // to be", and a window does not shrink mid-session, so it never falls.
    let s = fold(&format!(
        "{}\n{}",
        assistant_usage("m1", 500_000, 17_389),
        assistant_usage("m2", 50_000, 3_268),
    ));
    assert_eq!(s.context_used, 53_268, "the gauge follows the newest turn");
    assert_eq!(
        s.context_high_water, 517_389,
        "the evidence of the window's size does not expire with the reading"
    );

    // Rising is the ordinary case and must still track.
    let s = fold(&format!(
        "{}\n{}",
        assistant_usage("m1", 50_000, 3_268),
        assistant_usage("m2", 500_000, 17_389),
    ));
    assert_eq!(s.context_used, 517_389);
    assert_eq!(s.context_high_water, 517_389);

    // A session that never grew leaves the two equal — the high-water asserts nothing
    // extra, so it can never inflate a window on its own.
    let s = fold(&assistant_usage("m1", 50_000, 3_268));
    assert_eq!(s.context_used, s.context_high_water);
}
