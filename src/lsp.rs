//! Minimal Language Server Protocol front-end.
//!
//! The server intentionally supports the small surface that matters in an
//! editor: full-document sync, diagnostics, hover for AMXX natives/constants,
//! quick-fix suppressions and formatting. It speaks JSON-RPC over stdio and has
//! no daemon, so `zplint lsp` can be dropped into any LSP client.
//!
//! Framing is the LSP `Content-Length` header; messages are `serde_json`
//! values. Only stdout carries protocol frames - all human output stays off.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use serde_json::{Value, json};

use crate::config::Config;

pub fn run(cfg: &Config) -> i32 {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut stdout = std::io::stdout();
    let mut docs: HashMap<String, String> = HashMap::new();

    while let Some(body) = read_frame(&mut reader) {
        let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
            continue;
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").cloned();

        match method {
            "initialize" => {
                respond(
                    &mut stdout,
                    id,
                    json!({
                        "capabilities": {
                            "textDocumentSync": 1,
                            "hoverProvider": true,
                            "codeActionProvider": true,
                            "documentFormattingProvider": true
                        },
                        "serverInfo": { "name": "zplint", "version": env!("CARGO_PKG_VERSION") }
                    }),
                );
            }
            "initialized" | "$/setTrace" | "workspace/didChangeConfiguration" => {}
            "shutdown" => {
                respond(&mut stdout, id, Value::Null);
            }
            "exit" => break,
            "textDocument/didOpen" => {
                if let Some((uri, text)) = doc_text(&msg) {
                    docs.insert(uri.clone(), text);
                    publish(&mut stdout, &cfg.rules, &uri, docs.get(&uri).unwrap());
                }
            }
            "textDocument/didChange" => {
                if let Some(uri) = msg
                    .pointer("/params/textDocument/uri")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    && let Some(text) = msg
                        .pointer("/params/contentChanges")
                        .and_then(Value::as_array)
                        .and_then(|a| a.last())
                        .and_then(|c| c.get("text"))
                        .and_then(Value::as_str)
                {
                    docs.insert(uri.clone(), text.to_string());
                    publish(&mut stdout, &cfg.rules, &uri, text);
                }
            }
            "textDocument/didSave" => {
                if let Some(uri) = msg
                    .pointer("/params/textDocument/uri")
                    .and_then(Value::as_str)
                    && let Some(text) = docs.get(uri)
                {
                    publish(&mut stdout, &cfg.rules, uri, text);
                }
            }
            "textDocument/hover" => {
                let result = hover(&msg, &docs);
                respond(&mut stdout, id, result);
            }
            "textDocument/codeAction" => {
                let result = code_actions(&msg, &docs);
                respond(&mut stdout, id, result);
            }
            "textDocument/formatting" => {
                let result = formatting(&msg, &docs);
                respond(&mut stdout, id, result);
            }
            _ => {
                // Unknown notifications are ignored; unknown requests get null
                // rather than an error so older clients stay usable.
                if id.is_some() {
                    respond(&mut stdout, id, Value::Null);
                }
            }
        }
    }
    0
}

fn read_frame(reader: &mut BufReader<std::io::StdinLock<'_>>) -> Option<Vec<u8>> {
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            len = v.trim().parse().ok()?;
        }
    }
    if len == 0 || len > 64 * 1024 * 1024 {
        return None;
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn send(out: &mut std::io::Stdout, value: &Value) {
    let body = value.to_string();
    write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body).ok();
    out.flush().ok();
}

fn respond(out: &mut std::io::Stdout, id: Option<Value>, result: Value) {
    let Some(id) = id else { return };
    send(
        out,
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    );
}

fn notify(out: &mut std::io::Stdout, method: &str, params: Value) {
    send(
        out,
        &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
    );
}

fn doc_text(msg: &Value) -> Option<(String, String)> {
    let uri = msg
        .pointer("/params/textDocument/uri")?
        .as_str()?
        .to_string();
    let text = msg
        .pointer("/params/textDocument/text")?
        .as_str()?
        .to_string();
    Some((uri, text))
}

fn publish(out: &mut std::io::Stdout, rules: &crate::config::RulesConfig, uri: &str, text: &str) {
    let path = uri_to_path(uri);
    let issues = crate::engine::lint_source(Path::new(&path), text, rules);
    let mut diags = Vec::with_capacity(issues.len());
    for issue in issues {
        let line = issue.lineno.saturating_sub(1) as u64;
        let line_len = text
            .lines()
            .nth(line as usize)
            .map(|l| l.len())
            .unwrap_or(0) as u64;
        diags.push(json!({
            "range": { "start": { "line": line, "character": 0 }, "end": { "line": line, "character": line_len } },
            "severity": if issue.severity == crate::rules::Severity::Error { 1 } else { 2 },
            "code": issue.rule_id,
            "source": "zplint",
            "message": issue.message,
        }));
    }
    notify(
        out,
        "textDocument/publishDiagnostics",
        json!({ "uri": uri, "diagnostics": diags }),
    );
}

fn hover(msg: &Value, docs: &HashMap<String, String>) -> Value {
    let Some(uri) = msg
        .pointer("/params/textDocument/uri")
        .and_then(Value::as_str)
    else {
        return Value::Null;
    };
    let Some(text) = docs.get(uri) else {
        return Value::Null;
    };
    let Some(word) = word_at(msg, text) else {
        return Value::Null;
    };

    if let Some(f) = crate::api::lookup(&word) {
        let params: Vec<String> = f
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut s = String::new();
                if p.is_array {
                    s.push_str(&format!("arg{}[]", i + 1));
                } else {
                    s.push_str(&format!("arg{}", i + 1));
                }
                if p.optional {
                    s.push_str(" = ...");
                }
                s
            })
            .collect();
        let ret = if f.ret_tag == "_" {
            String::new()
        } else {
            format!("{}:", f.ret_tag)
        };
        let mut md = format!("```pawn\n{}{}({})\n```", ret, f.name, params.join(", "));
        md.push_str(&format!("\n\nAMX Mod X native from `<{}>`.", f.include));
        if let Some(rep) = f.deprecated {
            md.push_str(&format!("\n\n**Deprecated:** {rep}"));
        }
        return json!({ "contents": { "kind": "markdown", "value": md } });
    }
    if let Some((name, value)) = crate::api::CONSTANTS.iter().find(|(n, _)| *n == word) {
        return json!({
            "contents": { "kind": "markdown", "value": format!("```pawn\n#define {} {}\n```", name, value) }
        });
    }
    Value::Null
}

fn code_actions(msg: &Value, docs: &HashMap<String, String>) -> Value {
    let Some(uri) = msg
        .pointer("/params/textDocument/uri")
        .and_then(Value::as_str)
    else {
        return json!([]);
    };
    let Some(text) = docs.get(uri) else {
        return json!([]);
    };
    let Some(diags) = msg
        .pointer("/params/context/diagnostics")
        .and_then(Value::as_array)
    else {
        return json!([]);
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut actions = Vec::new();
    for d in diags {
        let Some(rule) = d.get("code").and_then(Value::as_str) else {
            continue;
        };
        let Some(line) = d.pointer("/range/start/line").and_then(Value::as_u64) else {
            continue;
        };
        let Some(src) = lines.get(line as usize) else {
            continue;
        };
        let end = src.len() as u64;
        actions.push(json!({
            "title": format!("Suppress {rule} with zplint-ignore"),
            "kind": "quickfix",
            "diagnostics": [d],
            "edit": {
                "changes": {
                    uri: [{
                        "range": {
                            "start": { "line": line, "character": end },
                            "end": { "line": line, "character": end }
                        },
                        "newText": format!(" // zplint-ignore {rule}")
                    }]
                }
            }
        }));
    }
    Value::Array(actions)
}

fn formatting(msg: &Value, docs: &HashMap<String, String>) -> Value {
    let Some(uri) = msg
        .pointer("/params/textDocument/uri")
        .and_then(Value::as_str)
    else {
        return Value::Null;
    };
    let Some(text) = docs.get(uri) else {
        return Value::Null;
    };
    let formatted = crate::fmt::format_source(text);
    if formatted == *text {
        return json!([]);
    }
    let lines: Vec<&str> = text.lines().collect();
    let last = lines.len().saturating_sub(1) as u64;
    let last_len = lines.last().map(|l| l.len()).unwrap_or(0) as u64;
    json!([{
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": last, "character": last_len }
        },
        "newText": formatted
    }])
}

fn word_at(msg: &Value, text: &str) -> Option<String> {
    let line = msg.pointer("/params/position/line")?.as_u64()? as usize;
    let character = msg.pointer("/params/position/character")?.as_u64()? as usize;
    let src = text.lines().nth(line)?;
    let bytes = src.as_bytes();
    if character > bytes.len() {
        return None;
    }
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = character;
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = character;
    while end < bytes.len() && is_ident(bytes[end]) {
        end += 1;
    }
    if start == end {
        None
    } else {
        src.get(start..end).map(str::to_string)
    }
}

fn uri_to_path(uri: &str) -> String {
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    // Windows file URIs are `file:///C:/...`; Unix paths keep their leading `/`.
    let path = if path.len() > 3 && path.as_bytes()[0] == b'/' && path.as_bytes()[2] == b':' {
        &path[1..]
    } else {
        path
    };
    percent_decode(path)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_and_word_helpers() {
        assert_eq!(
            uri_to_path("file:///C:/cs/plugins/a.sma"),
            "C:/cs/plugins/a.sma"
        );
        assert_eq!(uri_to_path("file:///tmp/a%20b.sma"), "/tmp/a b.sma");
        let text = "client_print(id, print_chat, \"%s\", name)";
        let msg = json!({"params": {"position": {"line": 0, "character": 4}, "textDocument": {"uri": "file:///a.sma"}}});
        assert_eq!(word_at(&msg, text).as_deref(), Some("client_print"));
    }
}
