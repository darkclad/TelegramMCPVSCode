//! Unit-tests the `pick_record` selection logic over an in-memory
//! list of records. No real filesystem access.

use local_pipe::DiscoveryRecord;
use tg_hook::discovery::pick_record;

#[allow(
    clippy::similar_names,
    reason = "pid/ppid are standard OS terminology in this domain"
)]
fn rec(pid: u32, ppid: u32, session: Option<&str>) -> DiscoveryRecord {
    DiscoveryRecord {
        pid,
        ppid,
        pipe: format!(r"\\.\pipe\telegrammcp-{pid}"),
        token: "t".into(),
        session_id: session.map(str::to_string),
        started_at: "2026-05-14T00:00:00Z".into(),
    }
}

#[test]
fn session_id_match_wins() {
    let records = vec![
        rec(100, 9999, Some("OTHER")),
        rec(200, 9999, Some("MINE")),
        rec(300, 9999, None),
    ];
    let pid_chain = vec![9999, 1];
    let picked = pick_record(&records, Some("MINE"), &pid_chain).expect("found");
    assert_eq!(picked.pid, 200);
}

#[test]
fn falls_back_to_ppid_chain_when_no_session_id() {
    let records = vec![
        rec(100, 7777, None),
        rec(200, 8888, None),
        rec(300, 9999, None),
    ];
    // Hook's parent chain: hook -> some-wrapper(8888) -> claude(7777) -> ...
    let pid_chain = vec![8888, 7777, 1];
    let picked = pick_record(&records, None, &pid_chain).expect("found");
    // 7777 is closer to root, but 8888 is the nearer match — we want the
    // FIRST match walking up the chain so nested wrappers stay deterministic.
    assert_eq!(picked.pid, 200);
}

#[test]
fn no_match_returns_none() {
    let records = vec![rec(100, 7777, Some("OTHER"))];
    let pid_chain = vec![9999, 1];
    assert!(pick_record(&records, Some("NOTPRESENT"), &pid_chain).is_none());
}
