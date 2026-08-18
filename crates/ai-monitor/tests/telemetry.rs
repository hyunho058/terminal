//! Headless assertions for the `telemetry-model` fold.
//!
//! Every input is either an inline literal or the checked-in fixture at
//! `tests/fixtures/session.jsonl` — never a live log, and no clock is read, so the
//! numbers are the same on every machine and every run (R10.1).

use ai_monitor::telemetry::{
    classify, FileOp, LogFormat, SessionTelemetry, TelemetrySnapshot, ToolKind,
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

// ---- ai-monitor-antigravity: the second dialect (AG2/AG3/AG4/AG5/AG9) ---------
//
// Every fixture below is SYNTHETIC. It reproduces the measured envelope exactly —
// the five universal keys, the dispatch/result pair, the double-encoded `args` — with
// invented paths and prompts, because a real `brain/<uuid>` transcript is somebody's
// prompts and somebody's project layout and this repo is version-controlled.

/// `2026-08-14T03:00:00Z` — the base the minute-per-step helper below counts from.
const AG_T0: u64 = 1_786_676_400_000;
/// The fixture's first `created_at`, `2026-08-14T03:48:57Z`.
const AG_FIX_T0: u64 = 1_786_679_337_000;

fn ag_fold(lines: &str) -> TelemetrySnapshot {
    let mut t = SessionTelemetry::with_format(LogFormat::Antigravity);
    for l in lines.lines() {
        t.apply(l);
    }
    t.snapshot()
}

/// One step in the measured envelope: `step_index`, `source`, `type`, `status` and
/// `created_at` are on every one of the 1,761 corpus steps, so they are on every one
/// here. `extra` is raw JSON appended to the object and must start with a comma.
///
/// One step per minute, so a dispatch/result pair is 60_000ms apart and a duration
/// assertion reads as arithmetic rather than as a magic number.
fn ag_step(i: u64, ty: &str, extra: &str) -> String {
    format!(
        r#"{{"step_index":{i},"source":"MODEL","type":"{ty}","status":"DONE","created_at":"2026-08-14T03:{:02}:00Z"{extra}}}"#,
        i
    )
}

/// A `PLANNER_RESPONSE` dispatching one tool call. `args` is the raw body of the args
/// object — written with its real double-encoding, which is the whole point of R2.3.
fn ag_dispatch(i: u64, name: &str, args: &str) -> String {
    ag_step(
        i,
        "PLANNER_RESPONSE",
        &format!(r#","tool_calls":[{{"name":"{name}","args":{{{args}}}}}]"#),
    )
}

/// A dispatch/result pair, the shape every Antigravity tool call actually takes.
fn ag_call(i: u64, name: &str, result_step: &str, args: &str) -> String {
    format!(
        "{}\n{}",
        ag_dispatch(i, name, args),
        ag_step(i + 1, result_step, "")
    )
}

#[test]
fn antigravity_fixture_folds_to_expected_telemetry() {
    // The end-to-end shape: two human turns, two replies, six tool calls each written
    // twice, a defined-then-invoked subagent, and a checkpoint that folds nothing.
    let s = ag_fold(include_str!("fixtures/antigravity_transcript.jsonl"));

    // AG3 — 2 user prompts, 2 tool-less planner replies. The other 6 planner steps
    // are dispatch. Counting all 8 is the 15:1 bug the ratio exists to catch.
    assert_eq!(s.turns_user, 2);
    assert_eq!(s.turns_assistant, 2);

    // AG4 — the model is prose in a settings sentence, tier retained, period dropped.
    assert_eq!(s.model.as_deref(), Some("Gemini 3.6 Flash (High)"));
    // The transcript names no session: the conversation id is the DIRECTORY's name,
    // which only the resolver (T1) knows. `None` is the honest value here.
    assert_eq!(s.session_id, None);

    // R3.1 — from the `Z`-suffixed step field, never the local-offset `Created At:`
    // lines inside `content`. Those read 20:48 where these read 03:48.
    assert_eq!(s.first_event_ms, Some(AG_FIX_T0));
    assert_eq!(s.last_event_ms, Some(AG_FIX_T0 + 23_000));

    // AG2 — six calls, each counted once at its result step. Twelve is what counting
    // the dispatch as well produces.
    assert_eq!(s.total_tool_calls, 6);
    assert_eq!(
        s.tool_calls,
        vec![
            ("define_subagent".to_string(), 1),
            ("grep_search".to_string(), 1),
            ("invoke_subagent".to_string(), 1),
            ("list_dir".to_string(), 1),
            ("run_command".to_string(), 1),
            ("view_file".to_string(), 1),
        ],
        "tools keep the names the LOG supplied — no Claude vocabulary"
    );
    assert_eq!(
        s.failed_tool_calls, 0,
        "G4 leaves the error family unmapped"
    );
    assert_eq!(s.parse_errors, 0, "the whole fixture is well-formed");
    assert_eq!(s.orphan_tool_results, 0, "every result found its dispatch");

    // FILE ACTIVITY, newest first, paths unquoted.
    let files: Vec<(FileOp, &str)> = s.files.iter().map(|f| (f.op, f.path.as_str())).collect();
    assert_eq!(
        files,
        vec![
            (FileOp::Grep, "/proj/crates/renderer"),
            (FileOp::List, "/proj/crates/renderer"),
            (FileOp::Read, "/proj/crates/renderer/atlas.rs"),
        ]
    );
    // Duration spans the dispatch and its result step, both on the same UTC clock.
    assert_eq!(s.files[2].duration_ms, Some(1_000));
    assert_eq!(s.files[2].start_ms, Some(AG_FIX_T0 + 1_000));

    // RECENT CALLS displays `toolSummary`, which is on every measured call.
    let tools: Vec<&str> = s.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        tools,
        vec![
            "Define the code-reviewer subagent",
            "Run the renderer tests"
        ]
    );
    assert_eq!(s.tools[1].duration_ms, Some(4_000));

    // AG5 — named by the preceding definition, joined to the child by conversation id.
    assert_eq!(s.subagents.len(), 1);
    let a = &s.subagents[0];
    assert_eq!(a.subagent_type.as_deref(), Some("code-reviewer"));
    assert_eq!(
        a.description.as_deref(),
        Some("Reviews a diff for correctness")
    );
    assert_eq!(a.id, "ed14342c-0000-4000-8000-00000000beef");
    assert!(a.completed, "the step's status is DONE");
    assert_eq!(a.end_ms, Some(AG_FIX_T0 + 14_000));

    // R2.6 — the child has its own `USER_INPUT`; folding it would double the turns.
    // Nothing here reads it, so the parent's count is the parent's alone.
    assert_eq!(s.turns_user, 2, "one invocation, still two human prompts");

    // AG4/G1 — the format carries no token accounting at all, and nothing here may
    // invent one from content lengths.
    assert_eq!(
        (
            s.total_input,
            s.total_output,
            s.total_cache_read,
            s.total_cache_creation,
            s.context_used,
            s.context_high_water
        ),
        (0, 0, 0, 0, 0, 0)
    );
    // G5 — `manage_task` implies a task model the transcript never records.
    assert!(s.tasks.is_empty());
    // No Antigravity tool maps to `Skill`.
    assert!(s.skills.is_empty());
}

#[test]
fn r2_1_a_tool_bearing_planner_step_is_not_a_turn() {
    // AG3, at its exact boundary. Measured 54 user inputs against 61 tool-less planner
    // responses and 772 tool-bearing ones — so this single branch is the difference
    // between a 1.13 ratio and a 15:1 one.
    let s = ag_fold(&format!(
        "{}\n{}\n{}",
        ag_step(1, "USER_INPUT", r#","content":"do the thing""#),
        ag_dispatch(2, "view_file", r#""AbsolutePath":"\"/a/b.rs\"""#),
        ag_step(3, "PLANNER_RESPONSE", r#","content":"done""#),
    ));
    assert_eq!(s.turns_user, 1);
    assert_eq!(
        s.turns_assistant, 1,
        "the dispatch increments no turn counter"
    );

    // An EMPTY `tool_calls` array dispatched nothing, so it is a reply like any other.
    let s = ag_fold(&ag_step(1, "PLANNER_RESPONSE", r#","tool_calls":[]"#));
    assert_eq!(s.turns_assistant, 1);
    assert_eq!(s.total_tool_calls, 0);
}

#[test]
fn r2_2_each_tool_call_is_counted_exactly_once() {
    // The single largest correctness trap in the format: every call is written twice,
    // once as a `tool_calls` dispatch and once as its own result step. The corpus holds
    // 773 dispatch entries against 754 result steps, so counting both nearly doubles
    // every tally the panel shows.
    let s = ag_fold(&ag_call(
        1,
        "view_file",
        "VIEW_FILE",
        r#""AbsolutePath":"\"/a/b.rs\"""#,
    ));
    assert_eq!(s.total_tool_calls, 1);
    assert_eq!(s.tool_calls, vec![("view_file".to_string(), 1)]);
    assert_eq!(s.files.len(), 1);

    // The dispatch ALONE counts nothing — an in-flight call is not a completed one.
    let s = ag_fold(&ag_dispatch(
        1,
        "view_file",
        r#""AbsolutePath":"\"/a/b.rs\"""#,
    ));
    assert_eq!(s.total_tool_calls, 0);
    assert!(s.files.is_empty());
}

#[test]
fn r2_3_paths_are_unwrapped_from_their_json_encoding() {
    // Every `args` value is a String whose CONTENTS are a JSON document (3,470 of
    // 3,470 measured values). Reading the value directly renders the wrapper quotes,
    // so every path on the panel would come out as `"/a/b.rs"`.
    let s = ag_fold(
        &[
            ag_call(
                1,
                "view_file",
                "VIEW_FILE",
                r#""AbsolutePath":"\"/a/b.rs\"""#,
            ),
            ag_call(
                3,
                "write_to_file",
                "CODE_ACTION",
                r#""TargetFile":"\"/a/c.rs\"""#,
            ),
            ag_call(5, "grep_search", "GREP_SEARCH", r#""SearchPath":"\"/a\"""#),
            ag_call(
                7,
                "list_dir",
                "LIST_DIRECTORY",
                r#""DirectoryPath":"\"/a\"""#,
            ),
        ]
        .join("\n"),
    );
    let files: Vec<(FileOp, &str)> = s.files.iter().map(|f| (f.op, f.path.as_str())).collect();
    assert_eq!(
        files,
        vec![
            (FileOp::List, "/a"),
            (FileOp::Grep, "/a"),
            (FileOp::Write, "/a/c.rs"),
            (FileOp::Read, "/a/b.rs"),
        ],
        "newest first"
    );
    assert!(
        s.files.iter().all(|f| !f.path.contains('"')),
        "no path may carry a literal quote"
    );
    assert_eq!(s.parse_errors, 0);
}

#[test]
fn r2_3_writes_arrive_as_code_action_and_finds_as_find() {
    // There is no `WRITE_FILE` step type — the first draft of the spec mapped one, and
    // the corpus has none. Writes land as `CODE_ACTION` under two different tool names,
    // and `find_by_name` reports through `FIND` while listing a directory.
    let s = ag_fold(
        &[
            ag_call(
                1,
                "replace_file_content",
                "CODE_ACTION",
                r#""TargetFile":"\"/a/d.rs\"""#,
            ),
            ag_call(3, "find_by_name", "FIND", r#""SearchDirectory":"\"/a\"""#),
        ]
        .join("\n"),
    );
    let files: Vec<(FileOp, &str)> = s.files.iter().map(|f| (f.op, f.path.as_str())).collect();
    assert_eq!(
        files,
        vec![(FileOp::List, "/a"), (FileOp::Write, "/a/d.rs")]
    );

    // A `WRITE_FILE` step claims no pending call, so it folds nothing and is not even
    // an orphan — it is not a result step type this format has.
    let s = ag_fold(&format!(
        "{}\n{}",
        ag_dispatch(1, "write_to_file", r#""TargetFile":"\"/a/c.rs\"""#),
        ag_step(2, "WRITE_FILE", "")
    ));
    assert_eq!(s.total_tool_calls, 0);
    assert_eq!(s.orphan_tool_results, 0);
}

#[test]
fn r2_4_a_command_is_displayed_by_its_tool_summary() {
    // `toolSummary` is on all 773 measured calls and is written for a human, so nothing
    // has to be synthesised from the tool name for RECENT CALLS. The tally still keys
    // on the NAME, so the two views can never disagree about what ran.
    let s = ag_fold(&ag_call(
        1,
        "run_command",
        "RUN_COMMAND",
        r#""CommandLine":"\"cargo test\"","Cwd":"\"/proj\"","WaitMsBeforeAsync":"5000","toolSummary":"\"Run the test suite\"""#,
    ));
    assert_eq!(s.tool_calls, vec![("run_command".to_string(), 1)]);
    assert_eq!(s.tools.len(), 1);
    assert_eq!(s.tools[0].name, "Run the test suite");
    assert_eq!(s.tools[0].duration_ms, Some(60_000));

    // Without one, the ring names the tool rather than showing nothing.
    let s = ag_fold(&ag_call(
        1,
        "run_command",
        "RUN_COMMAND",
        r#""Cwd":"\"/proj\"""#,
    ));
    assert_eq!(s.tools[0].name, "run_command");
    assert_eq!(
        s.parse_errors, 0,
        "a missing display label is not a parse fault — the call still names itself"
    );
}

#[test]
fn r2_5_a_subagent_is_named_by_the_preceding_definition() {
    // AG5 — `invoke_subagent`'s args carry only `Subagents[].Model` and `.Prompt`. The
    // name exists only on the `define_subagent` that precedes it, and the child's
    // conversation id exists only in the invocation's result `content`.
    let s = ag_fold(&format!(
        "{}\n{}\n{}",
        ag_call(
            1,
            "define_subagent",
            "GENERIC",
            r#""name":"\"code-reviewer\"","description":"\"Reviews a diff\"""#
        ),
        ag_dispatch(
            3,
            "invoke_subagent",
            r#""Subagents":"[{\"Model\":\"gemini\",\"Prompt\":\"review\"}]""#
        ),
        ag_step(
            4,
            "INVOKE_SUBAGENT",
            r#","content":"Created the following subagents:\n{\n  \"conversationId\": \"ed14342c-0000-4000-8000-00000000beef\"\n}""#
        ),
    ));
    assert_eq!(s.subagents.len(), 1, "the invocation, not the definition");
    let a = &s.subagents[0];
    assert_eq!(a.subagent_type.as_deref(), Some("code-reviewer"));
    assert_eq!(a.description.as_deref(), Some("Reviews a diff"));
    assert_eq!(
        a.id, "ed14342c-0000-4000-8000-00000000beef",
        "the join key to the child's own transcript, which AG5 does not tail"
    );
    assert!(a.completed, "status DONE");
    // Both calls are still calls, under their own names.
    assert_eq!(s.total_tool_calls, 2);

    // A `RUNNING` invocation is not finished, and its timer must not be frozen at a
    // duration it never took.
    let s = ag_fold(&format!(
        "{}\n{}",
        ag_dispatch(1, "invoke_subagent", r#""Subagents":"[]""#),
        r#"{"step_index":2,"source":"MODEL","type":"INVOKE_SUBAGENT","status":"RUNNING","created_at":"2026-08-14T03:02:00Z","content":"{\"conversationId\": \"abc\"}"}"#,
    ));
    assert!(!s.subagents[0].completed);
    assert_eq!(s.subagents[0].end_ms, None);
}

#[test]
fn r2_6_turn_counts_are_not_multiplied_by_subagents() {
    // AG5/R2.6 — a subagent gets its OWN conversation directory under `brain/`, and its
    // step 0 is a `USER_INPUT` / `USER_EXPLICIT` carrying a `<USER_REQUEST>` envelope
    // indistinguishable from a human's: the prompt text is simply the one the parent
    // wrote. 18 of the 51 measured transcripts are such children, with no marker
    // separating them from top-level conversations. So "count `USER_INPUT` steps" is
    // only a turn count for as long as the fold reads ONE transcript.
    const CHILD: &str = "ed14342c-0000-4000-8000-00000000beef";
    let parent = [
        ag_step(1, "USER_INPUT", r#","content":"<USER_REQUEST>\nShip it\n</USER_REQUEST>""#),
        // define -> invoke -> a lifecycle notice, the full subagent sequence. Every one
        // of these is a dispatch or a result step; none of them is a turn.
        ag_dispatch(2, "define_subagent", r#""name":"\"reviewer\"""#),
        ag_step(3, "GENERIC", r#","content":"Subagent \"reviewer\" defined.""#),
        ag_dispatch(4, "invoke_subagent", r#""Subagents":"[]""#),
        ag_step(
            5,
            "INVOKE_SUBAGENT",
            &format!(r#","content":"Created the following subagents:\n{{\n  \"conversationId\": \"{CHILD}\",\n  \"logAbsoluteUri\": \"file:///tmp/brain/{CHILD}\"\n}}""#),
        ),
        ag_dispatch(6, "send_message", r#""Recipient":"\"reviewer\"""#),
        ag_step(7, "GENERIC", &format!(r#","content":"Message sent to \"{CHILD}\".""#)),
        ag_step(8, "PLANNER_RESPONSE", r#","content":"Shipped.""#),
    ]
    .join("\n");

    let s = ag_fold(&parent);
    assert_eq!(
        s.turns_user, 1,
        "one human prompt, whatever the parent delegated"
    );
    assert_eq!(
        s.turns_assistant, 1,
        "one reply; the three dispatches and their results are not turns"
    );
    assert_eq!(s.total_tool_calls, 3, "the delegation is calls, not turns");

    // The child is NAMED, and named only. `apply` takes `&mut self` + `&str` and reads
    // no filesystem at all (D3), so `logAbsoluteUri` is inert text here — the id is a
    // join key the app may follow later, not a transcript this fold opens.
    assert_eq!(s.subagents.len(), 1);
    assert_eq!(s.subagents[0].id, CHILD);
    assert_eq!(s.subagents[0].subagent_type.as_deref(), Some("reviewer"));

    // And what interleaving would cost, as a number rather than as a warning. The
    // child's transcript opens with a `USER_INPUT` of its own, so folding it into this
    // accumulator doubles the human turn count for a session with one human prompt —
    // the exact corruption AG5 refuses child transcripts to avoid.
    let child = r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-08-14T03:09:00Z","content":"<USER_REQUEST>\nReview it\n</USER_REQUEST>"}"#;
    let both = ag_fold(&format!("{parent}\n{child}"));
    assert_eq!(both.turns_user, 2, "the count R2.6 exists to prevent");
}

#[test]
fn r2_7_malformed_antigravity_input_preserves_state() {
    // Constraint: no line, however malformed, may panic the fold or lose what was
    // already counted. Each line below is a different way to be wrong.
    let mut t = SessionTelemetry::with_format(LogFormat::Antigravity);
    for l in ag_call(
        1,
        "view_file",
        "VIEW_FILE",
        r#""AbsolutePath":"\"/a/b.rs\"""#,
    )
    .lines()
    {
        t.apply(l);
    }
    let before = t.snapshot();
    assert_eq!(before.total_tool_calls, 1);

    for bad in [
        "not json at all",
        r#"{"step_index":9,"type":"VIEW_FILE""#,
        // No `step_index`: the envelope is the format check, and half an envelope is
        // not something to guess the rest of.
        r#"{"type":"VIEW_FILE","status":"DONE","created_at":"2026-08-14T03:09:00Z"}"#,
        // `type` missing entirely.
        r#"{"step_index":9,"source":"MODEL","status":"DONE","created_at":"2026-08-14T03:09:00Z"}"#,
        // `tool_calls` present but not an array.
        &ag_step(
            9,
            "PLANNER_RESPONSE",
            r#","tool_calls":{"name":"view_file"}"#,
        ),
        // A call with no name.
        &ag_step(9, "PLANNER_RESPONSE", r#","tool_calls":[{"args":{}}]"#),
        // An `AbsolutePath` whose inner document is a bare number, not a string.
        &ag_dispatch(9, "view_file", r#""AbsolutePath":"100""#),
        // ... and one that is not a string at all.
        &ag_dispatch(9, "view_file", r#""AbsolutePath":100"#),
    ] {
        t.apply(bad);
    }
    let after = t.snapshot();
    assert_eq!(after.parse_errors, 8, "one per malformed line");
    assert_eq!(after.total_tool_calls, before.total_tool_calls);
    assert_eq!(after.files, before.files);
    assert_eq!(after.turns_user, before.turns_user);
    assert_eq!(after.turns_assistant, before.turns_assistant);

    // A call whose path could not be unwrapped still folds — dropping it would cost
    // the tally a real invocation to save one FILE ACTIVITY row.
    t.apply(&ag_step(10, "VIEW_FILE", ""));
    let s = t.snapshot();
    assert_eq!(s.total_tool_calls, 2);
    assert_eq!(s.files[0].path, "view_file", "the tool names itself");
}

#[test]
fn r2_8_a_claude_line_in_antigravity_format_is_a_parse_error() {
    // AG9 — a line from the wrong format is a bug upstream, not input to interpret.
    // Both dialects carry a `type` key, so folding what looks foldable would produce
    // real-looking telemetry for a session that never ran.
    let claude = concat!(
        r#"{"type":"assistant","sessionId":"5e59b629","timestamp":"2026-07-28T07:17:00.461Z","requestId":"r1","message":{"id":"m1","model":"claude-opus-5","usage":{"input_tokens":2,"output_tokens":441}}}"#,
        "\n",
        r#"{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/a/b.rs"}}"#,
        "\n",
        r#"{"type":"tool_result","tool_use_id":"t1"}"#,
    );
    let s = ag_fold(claude);
    assert_eq!(
        s.parse_errors, 3,
        "three well-formed Claude lines, three faults"
    );
    assert_eq!(
        TelemetrySnapshot {
            parse_errors: 0,
            ..s.clone()
        },
        TelemetrySnapshot::default(),
        "nothing else moved: no model, no tokens, no clock, no tool call"
    );
}

#[test]
fn the_format_is_fixed_at_construction() {
    // AG9, from both sides. The default is Claude, so every existing call site keeps
    // the behaviour it had.
    assert_eq!(SessionTelemetry::new().format(), LogFormat::Claude);
    assert_eq!(SessionTelemetry::default().format(), LogFormat::Claude);
    assert_eq!(
        SessionTelemetry::with_format(LogFormat::Antigravity).format(),
        LogFormat::Antigravity
    );

    // The mirror of R2.8: an Antigravity step fed to a Claude accumulator folds
    // nothing. Its `type` is not a Claude block type, its clock key is `created_at`,
    // and it has no `message`. A per-line sniff is what this pair makes unnecessary.
    let s = fold(&ag_step(1, "USER_INPUT", r#","content":"hello""#));
    assert_eq!(s, TelemetrySnapshot::default());
    assert_eq!(s.turns_user, 0, "the Claude fold populates neither counter");
}

#[test]
fn r3_1_timestamps_come_from_created_at_not_from_content() {
    // Antigravity's `created_at` is RFC 3339 with a `Z` and no fractional part — the
    // same shape `parse_ts` already reads for Claude, one field name away. The
    // `Created At:` line inside `content` is a LOCAL-offset clock seven hours away,
    // and folding it would shift every uptime by the machine's offset.
    let s = ag_fold(&format!(
        "{}\n{}",
        ag_step(
            5,
            "USER_INPUT",
            r#","content":"Created At: 2026-08-13T20:05:00-07:00\n""#
        ),
        ag_step(9, "PLANNER_RESPONSE", r#","content":"ok""#),
    ));
    assert_eq!(s.first_event_ms, Some(AG_T0 + 5 * 60_000));
    assert_eq!(s.last_event_ms, Some(AG_T0 + 9 * 60_000));

    // An unparseable `created_at` leaves the clock where it was rather than defaulting
    // to "now" — the step still folds, because the turn happened.
    let s = ag_fold(&format!(
        "{}\n{}",
        ag_step(5, "USER_INPUT", ""),
        r#"{"step_index":6,"source":"MODEL","type":"USER_INPUT","status":"DONE","created_at":"yesterday"}"#,
    ));
    assert_eq!(s.last_event_ms, Some(AG_T0 + 5 * 60_000));
    assert_eq!(s.turns_user, 2);
}

#[test]
fn r3_2_r3_3_the_last_model_selection_wins() {
    // The model is a sentence, not a field. The effort tier is part of the value; the
    // sentence's terminating period is not — and the value CONTAINS periods, so
    // cutting at the first one yields `Gemini 3`.
    let change = |i: u64, model: &str| {
        ag_step(
            i,
            "USER_INPUT",
            &format!(
                r#","content":"<USER_SETTINGS_CHANGE>\nThe user changed setting `Model Selection` from None to {model}. No need to comment on this change if the user doesn't ask about it.\n</USER_SETTINGS_CHANGE>""#
            ),
        )
    };
    let s = ag_fold(&change(1, "Gemini 3.6 Flash (High)"));
    assert_eq!(s.model.as_deref(), Some("Gemini 3.6 Flash (High)"));

    // R3.3 — last-wins, matching the existing field's contract.
    let s = ag_fold(&format!(
        "{}\n{}",
        change(1, "Gemini 3.6 Flash (High)"),
        change(2, "Gemini 3.7 Flash (High)")
    ));
    assert_eq!(s.model.as_deref(), Some("Gemini 3.7 Flash (High)"));

    // Only a USER_INPUT is authoritative: one measured `grep_search` result echoed a
    // settings sentence back, and scraping every step adopts a session's own log
    // output as its model.
    let echo = ag_step(
        1,
        "GREP_SEARCH",
        r#","content":"match: The user changed setting `Model Selection` from None to Gemini 9.9 Flash (Low). No need""#,
    );
    assert_eq!(ag_fold(&echo).model, None);
}

#[test]
fn r3_4_no_settings_block_leaves_the_model_unknown() {
    // 21 of 54 measured `USER_INPUT` steps carry no settings block at all, so this is
    // the common case, not the edge one. `None` renders the panel's existing
    // unknown-model form; a guessed default would be a claim nothing measured.
    let s = ag_fold(&format!(
        "{}\n{}",
        ag_step(
            1,
            "USER_INPUT",
            r#","content":"<USER_REQUEST>\nhi\n</USER_REQUEST>""#
        ),
        ag_step(2, "PLANNER_RESPONSE", r#","content":"hello""#),
    ));
    assert_eq!(s.model, None);
    assert_eq!(s.turns_user, 1, "the step still counts as a turn");
}

#[test]
fn an_unmapped_result_step_is_not_an_orphan_but_a_missing_dispatch_is() {
    // The two ways a result step can claim no pending call are different facts, and
    // `orphan_tool_results` names only the second. `ERROR_MESSAGE`, `CHECKPOINT`,
    // `SYSTEM_MESSAGE` and `CONVERSATION_HISTORY` report no tool call at all (G4
    // leaves the error family unmapped in this pass), so counting them would make the
    // diagnostic read as a parser fault on every well-formed transcript.
    let s = ag_fold(
        &[
            ag_step(1, "CHECKPOINT", ""),
            ag_step(2, "CONVERSATION_HISTORY", ""),
            ag_step(3, "SYSTEM_MESSAGE", ""),
            ag_step(4, "ERROR_MESSAGE", r#","error":"boom""#),
        ]
        .join("\n"),
    );
    assert_eq!(s.orphan_tool_results, 0);
    assert_eq!(s.parse_errors, 0);
    assert_eq!(s.total_tool_calls, 0);

    // A real orphan: the watcher starts from a bounded tail, so a result whose
    // dispatch sits above the window is guaranteed, not hypothetical.
    let s = ag_fold(&ag_step(1, "VIEW_FILE", ""));
    assert_eq!(s.orphan_tool_results, 1);
}

#[test]
fn the_antigravity_pending_queue_is_bounded() {
    // D6 is absolute, and this queue is the one structure the Antigravity fold adds
    // that a session can grow without bound: an async `run_command` interrupted before
    // its result step leaves an entry behind, and 7 such dispatches were in flight at
    // EOF across the measured corpus. Observable consequence of the cap: dispatch 70
    // calls, then answer all 70 — only the 64 the queue still holds can be matched.
    let mut t = SessionTelemetry::with_format(LogFormat::Antigravity);
    for i in 0..70 {
        t.apply(&ag_dispatch(
            i,
            "view_file",
            r#""AbsolutePath":"\"/a/b.rs\"""#,
        ));
    }
    assert_eq!(t.snapshot().total_tool_calls, 0, "premise: none completed");
    for i in 0..70 {
        t.apply(&ag_step(i, "VIEW_FILE", ""));
    }
    let s = t.snapshot();
    assert_eq!(s.total_tool_calls, 64, "PENDING_CAP, and not one more");
    assert_eq!(s.orphan_tool_results, 6, "the 6 evicted dispatches");
}

/// AG2's join rule, pinned by the one observable that distinguishes it: **duration**.
///
/// Two `run_command`s are in flight when a single `RUN_COMMAND` result arrives — the async
/// shape AG2 names, and the reason the rule is "oldest pending" rather than "the dispatch
/// just above me" (measured, nearest-preceding is right only 736 of 772 times).
///
/// Both rules produce the same `tool_calls` tally, the same `tools.len()`, and the same
/// names, so nothing about the *counts* can tell them apart. Only the pairing differs, and
/// only `duration_ms` exposes it: oldest-first pairs step 1 with step 5 (4 min), while
/// nearest-preceding pairs step 2 with step 5 (3 min). This test exists because mutating
/// `.position(` to `.rposition(` in `claim_pending` — inverting exactly this rule — left the
/// entire 1252-test suite green.
#[test]
fn an_async_result_claims_the_oldest_pending_call_not_the_nearest() {
    let s = ag_fold(
        &[
            // A long-running command dispatched first...
            ag_dispatch(
                1,
                "run_command",
                r#""CommandLine":"\"cargo test --workspace\"""#,
            ),
            // ...and a second one dispatched while the first is still running.
            ag_dispatch(2, "run_command", r#""CommandLine":"\"git status\"""#),
            // One result. Which call does it belong to?
            ag_step(5, "RUN_COMMAND", ""),
        ]
        .join("\n"),
    );

    assert_eq!(s.total_tool_calls, 1, "one result step, one counted call");
    assert_eq!(s.tools.len(), 1);
    assert_eq!(
        s.tools[0].duration_ms,
        Some(240_000),
        "the result must claim the OLDEST pending dispatch (step 1 -> 5 = 4min). \
         180_000 here means the join became nearest-preceding, which AG2 rejects"
    );
    assert_eq!(
        s.tools[0].start_ms,
        Some(AG_T0 + 60_000),
        "step 1's clock, not step 2's"
    );

    // And the queue drains in the same order: a second result takes the one left over.
    let s = ag_fold(
        &[
            ag_dispatch(1, "run_command", r#""CommandLine":"\"slow\"""#),
            ag_dispatch(2, "run_command", r#""CommandLine":"\"fast\"""#),
            ag_step(5, "RUN_COMMAND", ""),
            ag_step(6, "RUN_COMMAND", ""),
        ]
        .join("\n"),
    );
    assert_eq!(s.total_tool_calls, 2);
    let mut durs: Vec<Option<u64>> = s.tools.iter().map(|t| t.duration_ms).collect();
    durs.sort();
    assert_eq!(
        durs,
        vec![Some(240_000), Some(240_000)],
        "step1->5 is 4min and step2->6 is 4min; any other pair means the queue \
         did not drain oldest-first"
    );
}

#[test]
fn results_are_claimed_in_dispatch_order_by_expected_step_type() {
    // Antigravity's calls carry no id, so the pairing is by expected result-step type,
    // oldest first. That ordering is what an async `run_command` needs — its result
    // step can arrive several steps after a later call's — and `GENERIC` is what makes
    // it load-bearing rather than incidental: five different tools report through it.
    let s = ag_fold(
        &[
            ag_dispatch(1, "define_subagent", r#""name":"\"first\"""#),
            ag_dispatch(2, "manage_task", r#""Action":"\"create\"""#),
            ag_step(3, "GENERIC", ""),
            ag_step(4, "GENERIC", ""),
        ]
        .join("\n"),
    );
    assert_eq!(
        s.tool_calls,
        vec![
            ("define_subagent".to_string(), 1),
            ("manage_task".to_string(), 1)
        ]
    );
    // G5 — `manage_task` is classified `Task`, so it reaches neither the tools ring
    // nor the TASKS map: the transcript records no subject and no status transitions.
    assert_eq!(s.tools.len(), 1, "only the Plain `define_subagent`");
    assert!(s.tasks.is_empty());

    // A name the mapping does not know is never matched to a result step. Counting it
    // would mean crediting a real call to the wrong tool; the miss is visible instead.
    let s = ag_fold(&format!(
        "{}\n{}",
        ag_dispatch(1, "brand_new_tool", r#""toolSummary":"\"Do a new thing\"""#),
        ag_step(2, "VIEW_FILE", "")
    ));
    assert_eq!(s.total_tool_calls, 0);
    assert_eq!(s.orphan_tool_results, 1);
}

#[test]
fn the_turn_counters_default_to_nothing_folded() {
    // AG8 widens the versioned contract, and the app decides its empty state with
    // `*t == TelemetrySnapshot::default()`. A counter whose default meant anything
    // other than "not measured" would make an empty session read as an attached one.
    let d = TelemetrySnapshot::default();
    assert_eq!(d.turns_user, 0);
    assert_eq!(d.turns_assistant, 0);
    assert_eq!(
        SessionTelemetry::with_format(LogFormat::Antigravity).snapshot(),
        d,
        "a fresh Antigravity accumulator is indistinguishable from a fresh Claude one"
    );
}

/// G5 closed: an async command becomes a TASKS row and its status transitions (2026-08-16).
///
/// Bodies transcribed from a live 1.1.13 session, including the `Log:` line and the banner
/// that follow the fields — the parse is line-scoped precisely so those do not get swallowed.
///
/// Antigravity's "task" is an async background COMMAND, not a to-do item, and that is why
/// the card is worth filling: while a `cargo test --workspace` runs for minutes, it is the
/// one thing the session is actually waiting on.
#[test]
fn an_async_command_becomes_a_task_and_its_status_transitions() {
    let created = "Created At: 2026-08-16T12:08:57-07:00\nTool is running as a background \
task with task id: 6436db1a/task-39\nTask Description: cargo test --workspace\nTask logs \
are available at: file:///Users/x/.gemini/antigravity-cli/brain/6436db1a/.system_generated/\
tasks/task-39.log\nYOU MUST TRACK THIS TASK";
    let running = "Created At: 2026-08-16T12:09:10-07:00\nCompleted At: \
2026-08-16T12:09:10-07:00\nTask: 6436db1a/task-39\nStatus: RUNNING\nLog: /Users/x/t.log";
    let done = "Created At: 2026-08-16T12:09:21-07:00\nCompleted At: \
2026-08-16T12:09:21-07:00\nTask: 6436db1a/task-39\nStatus: DONE\nLog: /Users/x/t.log";

    let step = |i: u64, ty: &str, content: &str| {
        format!(
            r#"{{"step_index":{i},"source":"MODEL","type":"{ty}","status":"DONE","created_at":"2026-08-14T03:{:02}:00Z","content":{}}}"#,
            i,
            serde_json::to_string(content).unwrap()
        )
    };

    // born on an async run_command, then two manage_task status checks
    let s = ag_fold(
        &[
            ag_dispatch(
                1,
                "run_command",
                r#""CommandLine":"\"cargo test --workspace\"","WaitMsBeforeAsync":"8000""#,
            ),
            step(2, "RUN_COMMAND", created),
            ag_dispatch(
                3,
                "manage_task",
                r#""Action":"\"status\"","TaskId":"\"6436db1a/task-39\"""#,
            ),
            step(4, "GENERIC", running),
            ag_dispatch(
                5,
                "manage_task",
                r#""Action":"\"status\"","TaskId":"\"6436db1a/task-39\"""#,
            ),
            step(6, "GENERIC", done),
        ]
        .join("\n"),
    );

    assert_eq!(s.tasks.len(), 1, "one task, updated twice — not three rows");
    let (id, e) = &s.tasks[0];
    assert_eq!(id, "6436db1a/task-39", "keyed as manage_task spells it");
    assert_eq!(
        e.subject, "cargo test --workspace",
        "the subject is the command, which only the creation body carries"
    );
    assert_eq!(
        e.status, "completed",
        "RUNNING then DONE must land on the vocabulary the card colours green"
    );
    assert!(
        e.updated_ms > e.created_ms,
        "the last status moved the clock"
    );
    assert_eq!(s.orphan_task_updates, 0);

    // A status report with no creation above it must NOT invent a subject-less row.
    let s = ag_fold(
        &[
            ag_dispatch(
                1,
                "manage_task",
                r#""Action":"\"status\"","TaskId":"\"6436db1a/task-99\"""#,
            ),
            step(2, "GENERIC", "Task: 6436db1a/task-99\nStatus: RUNNING"),
        ]
        .join("\n"),
    );
    assert!(
        s.tasks.is_empty(),
        "a task whose creation sits above the tail window has no subject to show"
    );
    assert_eq!(
        s.orphan_task_updates, 1,
        "and the miss is recorded, not silent"
    );
}

/// G10 closed: an explicitly invoked skill becomes a SKILLS row (2026-08-16).
///
/// Body transcribed from the live corpus. Antigravity has no skill TOOL — the invocation is
/// a `<SKILL>` block spliced into the user's own turn — which is why an audit of tool names
/// concluded no skill signal existed. It does; it is just not where a tool would be.
#[test]
fn an_explicitly_invoked_skill_becomes_a_skills_row() {
    let with_skill = |i: u64, body: &str| {
        let content = format!(
            "<USER_REQUEST>\ndo the thing\n</USER_REQUEST>\n<ADDITIONAL_METADATA>\n{body}\n</ADDITIONAL_METADATA>"
        );
        format!(
            r#"{{"step_index":{i},"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-08-14T03:{:02}:00Z","content":{}}}"#,
            i,
            serde_json::to_string(&content).unwrap()
        )
    };

    let real = "<SKILL>The user has explicitly invoked the (worktree) skill. You must \
strictly follow the instructions in this skill to process their request. Here are its \
contents:\n# /worktree — Isolated Parallel Session Workspaces\n\nManage `git worktree` \
directories (see below).</SKILL>";

    let s = ag_fold(&with_skill(0, real));
    assert_eq!(s.skills.len(), 1, "the invocation must be recorded");
    assert_eq!(
        s.skills[0].name, "worktree",
        "the parenthesised name, not the body"
    );
    assert_eq!(
        s.skills[0].start_ms,
        Some(AG_T0),
        "stamped with the turn's clock"
    );
    assert_eq!(s.turns_user, 1, "and it is still one user turn");

    // The body's own parentheses must not be mistaken for the name — the real bodies are
    // full markdown documents and contain many.
    assert!(
        !s.skills[0].name.contains("see below"),
        "the scan must stop at the introducing sentence: {:?}",
        s.skills[0].name
    );

    // A turn with no skill block folds nothing.
    let s = ag_fold(&with_skill(
        0,
        "The current local time is: 2026-08-14T03:00:00-07:00.",
    ));
    assert!(s.skills.is_empty(), "no marker, no row");

    // A malformed block names nothing rather than inventing an unnamed row.
    let s = ag_fold(&with_skill(
        0,
        "<SKILL>The user has explicitly invoked the skill.</SKILL>",
    ));
    assert!(
        s.skills.is_empty(),
        "a marker with no name must fold nothing"
    );
}
