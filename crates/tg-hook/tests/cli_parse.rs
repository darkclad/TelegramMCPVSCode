//! Tests for the `CliArgs` argument parser.

use tg_hook::cli::CliArgs;

#[test]
fn parses_minimum_required() {
    let args = vec![
        "tg-hook".to_string(),
        "--chat".to_string(),
        "me".to_string(),
        "--message".to_string(),
        "Claude finished. Reply to continue.".to_string(),
    ];
    let cli = CliArgs::parse_from(args).expect("parses");
    assert_eq!(cli.chat, "me");
    assert_eq!(cli.message, "Claude finished. Reply to continue.");
    assert_eq!(cli.timeout_secs, 3600);
    assert_eq!(cli.poll_secs, 5);
    assert_eq!(cli.retry_message.as_deref(), None);
}

#[test]
fn parses_full() {
    let args = vec![
        "tg-hook".to_string(),
        "--chat".to_string(),
        "me".to_string(),
        "--message".to_string(),
        "wake".to_string(),
        "--retry-message".to_string(),
        "still waiting".to_string(),
        "--timeout-secs".to_string(),
        "60".to_string(),
        "--poll-secs".to_string(),
        "2".to_string(),
    ];
    let cli = CliArgs::parse_from(args).expect("parses");
    assert_eq!(cli.retry_message.as_deref(), Some("still waiting"));
    assert_eq!(cli.timeout_secs, 60);
    assert_eq!(cli.poll_secs, 2);
}

#[test]
fn missing_chat_errors() {
    let args = vec![
        "tg-hook".to_string(),
        "--message".to_string(),
        "x".to_string(),
    ];
    assert!(CliArgs::parse_from(args).is_err());
}

#[test]
fn missing_message_errors() {
    let args = vec![
        "tg-hook".to_string(),
        "--chat".to_string(),
        "me".to_string(),
    ];
    assert!(CliArgs::parse_from(args).is_err());
}

#[test]
fn unknown_flag_errors() {
    let args = vec!["tg-hook".to_string(), "--bogus".to_string()];
    assert!(CliArgs::parse_from(args).is_err());
}
