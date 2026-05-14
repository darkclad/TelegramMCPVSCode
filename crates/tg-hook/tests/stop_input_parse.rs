//! Verifies the Stop-hook stdin JSON parser ignores unknown fields and
//! decodes the three fields tg-hook actually consumes.

use tg_hook::stop_input::StopInput;

#[test]
fn decodes_known_fields_and_ignores_extras() {
    let raw = r#"{
        "session_id": "abc-123",
        "transcript_path": "C:\\path\\transcript.json",
        "stop_hook_active": true,
        "cwd": "D:/proj",
        "future_field": "ignored"
    }"#;
    let v: StopInput = serde_json::from_str(raw).expect("parses");
    assert_eq!(v.session_id.as_deref(), Some("abc-123"));
    assert_eq!(v.stop_hook_active, Some(true));
    assert_eq!(
        v.transcript_path.as_deref(),
        Some("C:\\path\\transcript.json")
    );
}

#[test]
fn all_fields_optional() {
    let v: StopInput = serde_json::from_str("{}").expect("parses");
    assert_eq!(v.session_id, None);
    assert_eq!(v.stop_hook_active, None);
}
