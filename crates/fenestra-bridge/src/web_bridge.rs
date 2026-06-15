//! Engine-neutral Fenestra bridge surface and the canonical bridge script.
//!
//! The script in `web_bridge.js` is the single source of truth for
//! `window.fenestra` inside every Fenestra webview. The Rust helpers in this
//! module let every backend (CEF and WebView2) build a fully-instantiated
//! install script for a given command allow-list, build the
//! `fenestra://bridge/<id>?...` URL that page-side `invoke()` produces, and
//! parse that URL back into a [`BridgeRequest`] on the host side.

use std::collections::BTreeSet;

/// The canonical Fenestra bridge script. The host (CEF or WebView2) is
/// expected to set `window.__fenestraBridgeCommands` to a JSON array of
/// allowed bridge command names before executing this script.
pub const INSTALL_SCRIPT: &str = include_str!("web_bridge.js");

/// Build the fully-instantiated Fenestra bridge install script for a given
/// command allow-list. The returned string is safe to feed straight to
/// `ExecuteJavaScript` (CEF) or `AddScriptToExecuteOnDocumentCreated`
/// (WebView2).
pub fn install_script(commands: &[&str]) -> String {
    let unique: BTreeSet<&str> = commands.iter().copied().collect();
    let json = unique
        .into_iter()
        .map(|name| {
            let escaped = name
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(",");
    let prelude = format!("window.__fenestraBridgeCommands=[{json}];");
    format!("{prelude}{INSTALL_SCRIPT}")
}

/// A bridge request parsed from a `fenestra://bridge/<id>?...` URL produced
/// by page-side `invoke()`. The host dispatches `name` to a registered
/// handler with `params`, then resolves the promise via
/// [`crate::bridge::BridgeRuntime::dispatch`] (which returns a
/// `BridgeResponse` to send back to the page) and finally posts the response
/// via `window.__fenestraBridgeResolve(id, ok, payload)`.
#[derive(Clone, Debug, PartialEq)]
pub struct BridgeRequest {
    pub id: String,
    pub command: crate::bridge::BridgeCommand,
}

/// The Fenestra bridge scheme used for host-side dispatch.
pub const BRIDGE_SCHEME: &str = "fenestra://bridge/";

/// The Fenestra window-control scheme (`fenestra://window/<action>?...`)
/// used by `window.fenestra.window.show()` etc.
pub const WINDOW_SCHEME: &str = "fenestra://window/";

/// Build the URL that page-side `invoke()` navigates to.
pub fn bridge_url(id: &str, name: &str, params: &serde_json::Value) -> String {
    let payload = serde_json::to_string(params).unwrap_or_else(|_| "null".to_string());
    let encoded_payload = url_encode(&payload);
    let encoded_name = url_encode(name);
    let encoded_id = url_encode(id);
    format!("{BRIDGE_SCHEME}{encoded_id}?name={encoded_name}&payload={encoded_payload}")
}

/// Parse a `fenestra://bridge/<id>?...` URL into a [`BridgeRequest`].
/// Returns `None` if the URL is not a bridge request or is missing
/// required parts.
pub fn parse_bridge_url(url: &str, origin: Option<&str>) -> Option<BridgeRequest> {
    let suffix = url.strip_prefix(BRIDGE_SCHEME)?;
    let (id, query) = match suffix.find('?') {
        Some(index) => (&suffix[..index], Some(&suffix[index + 1..])),
        None => (suffix, None),
    };
    if id.is_empty() {
        return None;
    }
    let id = url_decode(id);
    let query = query?;
    let mut name: Option<&str> = None;
    let mut payload: Option<&str> = None;
    for part in query.split('&') {
        if let Some(value) = part.strip_prefix("name=") {
            name = Some(value);
        } else if let Some(value) = part.strip_prefix("payload=") {
            payload = Some(value);
        }
    }
    let name = url_decode(name?);
    let raw_payload = url_decode(payload.unwrap_or("{}"));
    let params: serde_json::Value =
        serde_json::from_str(&raw_payload).unwrap_or(serde_json::Value::Object(Default::default()));
    Some(BridgeRequest {
        id,
        command: crate::bridge::BridgeCommand {
            name,
            params,
            origin: origin.map(str::to_string),
        },
    })
}

/// A window-control URL parsed from `fenestra://window/<action>?...`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowCommand {
    pub action: String,
}

impl WindowCommand {
    pub fn parse(url: &str) -> Option<Self> {
        let suffix = url.strip_prefix(WINDOW_SCHEME)?;
        let (action, _query) = match suffix.find('?') {
            Some(index) => (&suffix[..index], Some(&suffix[index + 1..])),
            None => (suffix, None),
        };
        if action.is_empty() {
            return None;
        }
        Some(Self {
            action: action.to_string(),
        })
    }
}

fn url_encode(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(*byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn url_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                output.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = match hex_digit(bytes[i + 1]) {
                    Some(value) => value,
                    None => {
                        output.push(bytes[i]);
                        i += 1;
                        continue;
                    }
                };
                let lo = match hex_digit(bytes[i + 2]) {
                    Some(value) => value,
                    None => {
                        output.push(bytes[i]);
                        i += 1;
                        continue;
                    }
                };
                output.push((hi << 4) | lo);
                i += 3;
            }
            byte => {
                output.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn install_script_substitutes_command_list() {
        let script = install_script(&["notes.list", "vault.unlock"]);
        assert!(script.starts_with("window.__fenestraBridgeCommands="));
        assert!(script.contains("\"notes.list\""));
        assert!(script.contains("\"vault.unlock\""));
        assert!(script.contains("fenestra://bridge/"));
    }

    #[test]
    fn bridge_url_round_trips() {
        let url = bridge_url("7", "notes.list", &json!({ "page": 2 }));
        let parsed = parse_bridge_url(&url, Some("file:///tmp/index.html"))
            .expect("bridge url should parse");
        assert_eq!(parsed.id, "7");
        assert_eq!(parsed.command.name, "notes.list");
        assert_eq!(parsed.command.params, json!({ "page": 2 }));
        assert_eq!(
            parsed.command.origin.as_deref(),
            Some("file:///tmp/index.html")
        );
    }

    #[test]
    fn window_command_parses() {
        let parsed = WindowCommand::parse("fenestra://window/show?at=1234-0.5").unwrap();
        assert_eq!(parsed.action, "show");
    }
}
