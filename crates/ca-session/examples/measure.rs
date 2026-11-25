//! Measures the session store on a long session: `cargo run --release -p ca-session --example measure`.
//! 10,000 messages (2,500 turns of a question, a tool call, its result and an answer), on disk.

use std::time::Instant;

use ca_llm::{Message, ToolCall};
use ca_session::Store;

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

fn main() {
    let dir = std::env::temp_dir().join(format!("ca-measure-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("sessions.db");
    let s = Store::open(&path).expect("open");
    let id = s.create("/w", "m", "ask").expect("create");
    let t = Instant::now();
    let result = "x".repeat(2_000);
    for n in 0..2_500 {
        s.append(&id, &Message::user(format!("question {n} about the parser and its tests")), true).expect("u");
        let mut a = Message::assistant("");
        a.tool_calls.push(ToolCall {
            id: format!("c{n}"),
            name: "read_file".into(),
            arguments: r#"{"path":"src/parser.rs"}"#.into(),
        });
        s.append(&id, &a, false).expect("a");
        s.append(&id, &Message::tool(format!("c{n}"), result.clone()), false).expect("t");
        s.append(&id, &Message::assistant(format!("answer {n}: the parser reads tokens")), false).expect("a");
    }
    let write_ms = t.elapsed().as_secs_f64() * 1000.0;
    s.compact(&id, 9_000, "## Goal\nsummary").expect("compact");
    drop(s);

    let mut open_page = Vec::new();
    let mut earlier = Vec::new();
    let mut model = Vec::new();
    let mut search = Vec::new();
    for _ in 0..20 {
        let t = Instant::now();
        let s = Store::open(&path).expect("open");
        let p = s.page(&id, 30, None).expect("page");
        open_page.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        s.page(&id, 30, Some(p.first_turn)).expect("page");
        earlier.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        s.model_view(&id).expect("view");
        model.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        s.search("/w", "parser tests", 20).expect("search");
        search.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        + std::fs::metadata(path.with_extension("db-wal")).map(|m| m.len()).unwrap_or(0);
    println!("messages                         10000 ({} MB on disk)", size / 1_000_000);
    println!("append, each (on disk)           {:.3} ms", write_ms / 10_000.0);
    println!("open and read the last 30 turns  {:.2} ms (median of 20)", median(open_page));
    println!("read 30 turns before those       {:.2} ms", median(earlier));
    println!("what the model reads (compacted) {:.2} ms", median(model));
    println!("search the folder's sessions     {:.2} ms", median(search));
    let _ = std::fs::remove_dir_all(dir);
}
