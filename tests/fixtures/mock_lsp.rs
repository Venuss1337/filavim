//! Standalone deterministic JSON-RPC fixture, compiled by integration tests.
use std::{
    fs::OpenOptions,
    io::{self, BufRead, Read, Write},
};
fn field<'a>(s: &'a str, key: &str) -> &'a str {
    let needle = format!("\"{key}\":\"");
    s.find(&needle)
        .and_then(|i| s[i + needle.len()..].split('"').next())
        .unwrap_or("")
}
fn id(s: &str) -> u64 {
    s.find("\"id\":")
        .map(|i| {
            s[i + 5..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}
fn send(value: &str) {
    let mut out = io::stdout().lock();
    write!(out, "Content-Length: {}\r\n\r\n{value}", value.len()).unwrap();
    out.flush().unwrap();
}
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).is_some_and(|a| a == "--terminal-output") {
        print!("\x1b[?1h\x1b[?2004h");
        for _ in 0..500 { println!("\x1b[32mheadless terminal output\x1b[0m"); io::stdout().flush().unwrap(); std::thread::sleep(std::time::Duration::from_millis(2)); }
        return;
    }
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args[1])
        .unwrap();
    let mut reader = io::stdin().lock();
    let mut uri = String::new();
    let mut slow = 0;
    let mut document_text = String::new();
    loop {
        let mut size = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 {
                return;
            }
            if line == "\r\n" {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length:") {
                size = v.trim().parse::<usize>().unwrap();
            }
        }
        let mut bytes = vec![0; size];
        reader.read_exact(&mut bytes).unwrap();
        let message = String::from_utf8(bytes).unwrap();
        writeln!(log, "{message}").unwrap();
        log.flush().unwrap();
        let method = field(&message, "method");
        let request = id(&message);
        if args.iter().any(|a| a == "--flood") && matches!(method, "textDocument/references" | "textDocument/codeAction") {
            let item = if method == "textDocument/references" { format!(r#"{{"uri":"{uri}","range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":1}}}}}}"#) } else { r#"{"title":"bulk action","kind":"quickfix"}"#.into() };
            let result = vec![item; 20_000].join(",");
            send(&format!(r#"{{"jsonrpc":"2.0","id":{request},"result":[{result}]}}"#));
            continue;
        }
        match method {
            "initialize" => {
                if args.iter().any(|a| a == "--crash") {
                    return;
                }
                if args.iter().any(|a| a == "--malformed") {
                    print!("Content-Length: 4\r\n\r\nnope");
                    io::stdout().flush().unwrap();
                    return;
                }
                if args.iter().any(|a| a == "--noinit") {
                    continue;
                }
                let mut capabilities=
                    r#"{"jsonrpc":"2.0","id":0,"result":{"capabilities":{"positionEncoding":"utf-16","textDocumentSync":1,"completionProvider":{"resolveProvider":true},"hoverProvider":true,"definitionProvider":true,"implementationProvider":true,"referencesProvider":true,"renameProvider":true,"codeActionProvider":{"resolveProvider":true},"documentFormattingProvider":true,"documentRangeFormattingProvider":true,"signatureHelpProvider":{},"executeCommandProvider":{"commands":["mock.fix"]}}}}"#.to_string();
                capabilities = capabilities.replace("\"signatureHelpProvider\":{}", "\"signatureHelpProvider\":{\"triggerCharacters\":[\"(\",\",\"]}");
                if args.iter().any(|a| a == "--incremental") {
                    capabilities = capabilities.replace(
                        "\"textDocumentSync\":1",
                        "\"textDocumentSync\":{\"openClose\":true,\"change\":2}",
                    );
                }
                if args.iter().any(|a| a == "--utf8") {
                    capabilities = capabilities.replace("utf-16", "utf-8");
                }
                send(&capabilities);
                if args.iter().any(|a| a == "--stuck") {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                    return;
                }
                send(
                    r#"{"jsonrpc":"2.0","id":900,"method":"workspace/configuration","params":{"items":[{},{}]}}"#,
                );
            }
            "textDocument/didOpen" => {
                document_text = field(&message, "text").into();
                uri = field(&message, "uri").into();
                if args.iter().any(|a| a == "--flood") {
                    let diagnostic = r#"{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"severity":1,"message":"large diagnostic fixture"}"#;
                    let diagnostics = vec![diagnostic; 20_000].join(",");
                    for _ in 0..if args.iter().any(|a| a == "--stream") { 24 } else { 3 } { send(&format!(r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":"{uri}","version":0,"diagnostics":[{diagnostics}]}}}}"#)); if args.iter().any(|a| a == "--stream") { std::thread::sleep(std::time::Duration::from_millis(50)); } }
                }
                send(&format!(
                    r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":"{uri}","version":0,"diagnostics":[{{"range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":1}}}},"severity":1,"message":"mock diagnostic"}}]}}}}"#
                ));
            }
            "textDocument/completion" => send(&format!(
                r#"{{"jsonrpc":"2.0","id":{request},"result":{{"isIncomplete":false,"items":[{{"label":"variable","sortText":"1","insertText":"${{1:variable}}$0","insertTextFormat":2,"detail":"Mock variable","documentation":{{"kind":"markdown","value":"Fixture docs"}}}}]}}}}"#
            )),
            "completionItem/resolve" => send(&format!(
                r#"{{"jsonrpc":"2.0","id":{request},"result":{{"label":"variable","insertText":"variable","detail":"Resolved detail","documentation":"Resolved docs"}}}}"#
            )),
            "textDocument/hover" => send(&format!(
                r#"{{"jsonrpc":"2.0","id":{request},"result":{{"contents":{{"kind":"markdown","value":"Mock hover documentation"}}}}}}"#
            )),
            "textDocument/signatureHelp" => send(&format!(
                r#"{{"jsonrpc":"2.0","id":{request},"result":{{"activeSignature":0,"activeParameter":{},"signatures":[{{"label":"f(first: i32, second: str) -> bool","parameters":[{{"label":"first: i32"}},{{"label":"second: str"}}]}}]}}}}"#,
                usize::from(document_text.contains(','))
            )),
            "textDocument/didChange" => document_text = field(&message, "text").into(),
            "textDocument/definition"
            | "textDocument/implementation"
            | "textDocument/references" => send(&format!(
                r#"{{"jsonrpc":"2.0","id":{request},"result":[{{"uri":"{uri}","range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":1}}}}}}]}}"#
            )),
            "textDocument/rename" => {
                let name = field(&message, "newName");
                send(&format!(
                    r#"{{"jsonrpc":"2.0","id":{request},"result":{{"changes":{{"{uri}":[{{"range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":1}}}},"newText":"{name}"}}]}}}}}}"#
                ));
            }
            "textDocument/formatting" | "textDocument/rangeFormatting" => send(&format!(
                r#"{{"jsonrpc":"2.0","id":{request},"result":[{{"range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":1}}}},"newText":"F"}}]}}"#
            )),
            "textDocument/codeAction" => send(&format!(
                r#"{{"jsonrpc":"2.0","id":{request},"result":[{{"title":"Mock quick fix","kind":"quickfix","edit":{{"changes":{{"{uri}":[{{"range":{{"start":{{"line":0,"character":0}},"end":{{"line":0,"character":1}}}},"newText":"Q"}}]}}}}}}]}}"#
            )),
            "slow" => slow = request,
            "$/cancelRequest" => {
                if slow != 0 {
                    send(&format!(
                        r#"{{"jsonrpc":"2.0","id":{slow},"result":"late"}}"#
                    ));
                    slow = 0;
                }
            }
            "askApply" => {
                send(
                    r#"{"jsonrpc":"2.0","id":901,"method":"workspace/applyEdit","params":{"edit":{"changes":{}}}}"#,
                );
                send(&format!(
                    r#"{{"jsonrpc":"2.0","id":{request},"result":null}}"#
                ));
            }
            "shutdown" => send(&format!(
                r#"{{"jsonrpc":"2.0","id":{request},"result":null}}"#
            )),
            "exit" => return,
            "initialized"
            | "textDocument/didClose"
            | "textDocument/didSave"
            | "" => {}
            _ => send(&format!(
                r#"{{"jsonrpc":"2.0","id":{request},"result":{{"ok":true}}}}"#
            )),
        }
    }
}
