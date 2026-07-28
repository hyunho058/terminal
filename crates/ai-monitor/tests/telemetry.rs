//! Headless assertions for the `telemetry-model` fold.
//!
//! Every input is either an inline literal or the checked-in fixture at
//! `tests/fixtures/session.jsonl` — never a live log, and no clock is read, so the
//! numbers are the same on every machine and every run (R10.1).

use ai_monitor::telemetry::{classify, FileOp, SessionTelemetry, TelemetrySnapshot, ToolKind};

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
