use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn zerograph() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_zerograph"));
    c.env_remove("ZEROGRAPH_BENCH_REPO").env_remove("REPOGRAPH_BENCH_REPO").env_remove("ZEROGRAPH_EMBED_MODEL").env_remove("REPOGRAPH_EMBED_MODEL").env_remove("ZEROGRAPH_CODE_GLOBS").env_remove("REPOGRAPH_CODE_GLOBS");
    c
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::fs::write(dir.path().join("docs/pay.md"), "**FR-PAY-1 · MUST · cancellation fee**\n\nThe fee is charged on a late cancellation.\n").unwrap();
    let built = zerograph().arg("--repo").arg(dir.path()).args(["--no-dense", "build"]).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    dir
}

/// The whole session over the pipes a harness uses: every line of stdout is one JSON message, and
/// closing stdin ends the server. `reindex` is left to the unit tests, since it detaches a
/// process that would outlive the directory.
#[test]
fn a_session_over_stdio_reaches_the_read_tools_and_stdout_is_only_protocol() {
    let dir = repo();
    let mut server = zerograph().arg("--repo").arg(dir.path()).args(["--no-dense", "mcp"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut stdin = server.stdin.take().unwrap();
    let call = |id: u32, name: &str, args: serde_json::Value| serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": args } });
    let messages = [
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } }),
        serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        call(3, "status", serde_json::json!({})),
        call(4, "ask", serde_json::json!({ "question": "cancellation fee", "stale": true })),
        call(5, "explain", serde_json::json!({ "node": "FR-PAY-1" })),
    ];
    for m in &messages { writeln!(stdin, "{m}").unwrap(); }
    writeln!(stdin, "not json").unwrap();
    drop(stdin);

    let replies: Vec<serde_json::Value> = BufReader::new(server.stdout.take().unwrap()).lines()
        .map(|l| serde_json::from_str(&l.unwrap()).expect("stdout carries protocol only"))
        .collect();
    assert!(server.wait().unwrap().success(), "end of input is a clean exit");
    assert_eq!(replies.len(), 6, "the notification gets none, the malformed line gets an error: {replies:?}");
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 9);
    let text = |i: usize| replies[i]["result"]["content"][0]["text"].as_str().unwrap().to_string();
    assert!(text(2).contains("refresh: none running"), "{}", text(2));
    assert!(text(3).contains("FR-PAY-1"), "{}", text(3));
    assert!(text(4).contains("cancellation fee"), "{}", text(4));
    assert_eq!(replies[5]["error"]["code"], -32700);
}
