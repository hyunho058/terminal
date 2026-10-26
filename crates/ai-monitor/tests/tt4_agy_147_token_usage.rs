use ai_monitor::telemetry::{LogFormat, SessionTelemetry};

#[test]
fn r4_1_cumulative_token_counting_from_planner_response() {
    let mut telemetry = SessionTelemetry::with_format(LogFormat::Antigravity);
    let step = r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-10-24T18:00:00Z","input_tokens":3415,"output_tokens":29,"cache_read_tokens":97161}"#;

    telemetry.apply(step);
    let s = telemetry.snapshot();

    assert_eq!(s.total_input, 3415);
    assert_eq!(s.total_output, 29);
    assert_eq!(s.total_cache_read, 97161);

    // Cumulative addition across subsequent PLANNER_RESPONSE turns
    let step2 = r#"{"step_index":2,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-10-24T18:01:00Z","input_tokens":1000,"output_tokens":50,"cache_read_tokens":2000}"#;
    telemetry.apply(step2);
    let s2 = telemetry.snapshot();

    assert_eq!(s2.total_input, 3415 + 1000);
    assert_eq!(s2.total_output, 29 + 50);
    assert_eq!(s2.total_cache_read, 97161 + 2000);
}

#[test]
fn r4_2_context_occupancy_from_planner_response() {
    let mut telemetry = SessionTelemetry::with_format(LogFormat::Antigravity);
    let step = r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-10-24T18:00:00Z","input_tokens":3415,"cache_read_tokens":97161}"#;

    telemetry.apply(step);
    let s = telemetry.snapshot();

    assert_eq!(s.context_used, 100576);
    assert!(s.context_high_water >= 100576);

    // Later turn with lower context verifies context_used is latest-value gauge
    // and context_high_water is monotonic max (D7)
    let step2 = r#"{"step_index":2,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-10-24T18:01:00Z","input_tokens":500,"cache_read_tokens":1000}"#;
    telemetry.apply(step2);
    let s2 = telemetry.snapshot();

    assert_eq!(s2.context_used, 1500);
    assert!(s2.context_high_water >= 100576);
}
