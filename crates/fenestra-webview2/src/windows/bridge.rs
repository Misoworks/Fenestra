// Bridge dispatch for the WebView2 backend.
//
// This module is loaded only on `target_os = "windows"`. It installs
// the WebView2 event handlers that the Fenestra bridge protocol
// needs:
//
// - `ICoreWebView2::add_NavigationStarting` — cancels the navigation
//   when the page navigates to a `fenestra://bridge/<id>?...` URL,
//   parses the URL, dispatches to the `BridgeRuntime`, and posts the
//   response back to the page via
//   `window.__fenestraBridgeResolve(id, ok, payload)` using
//   `ICoreWebView2::ExecuteScript`.
// - `ICoreWebView2::add_WebMessageReceived` — receives
//   `window.chrome.webview.postMessage(...)` payloads so the host
//   can react to plain-text commands posted by the page without
//   forcing a navigation.
//
// The bridge install script is registered once with
// `ICoreWebView2::AddScriptToExecuteOnDocumentCreated` so every
// fresh document gets `window.fenestra` defined before any user
// script runs. The actual call lives in `launch::create_webview2`.

#![cfg(target_os = "windows")]

use std::sync::Arc;

use fenestra_bridge::{BridgeResult, WindowCommand, parse_bridge_url};
use webview2_com::{
    AddScriptToExecuteOnDocumentCreatedCompletedHandler, ExecuteScriptCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2NavigationStartingEventArgs,
        ICoreWebView2WebMessageReceivedEventArgs,
    },
    NavigationStartingEventHandler, WebMessageReceivedEventHandler,
};
use windows::core::{PCWSTR, PWSTR};

use crate::{
    WebView2Error, WebView2ProcessInner, WebView2Result, windows::launch::WebView2UserEvent,
};

pub(crate) fn register_navigation_starting(
    webview: &ICoreWebView2,
    inner: Arc<WebView2ProcessInner>,
) -> WebView2Result<()> {
    let nav_inner = inner.clone();
    let handler = NavigationStartingEventHandler::create(Box::new(move |webview, args| {
        handle_navigation_starting(nav_inner.clone(), webview, args);
        Ok(())
    }));
    let mut token: i64 = 0;
    unsafe { webview.add_NavigationStarting(&handler, &mut token) }.map_err(webview2_error)?;
    inner.metrics.mark("nav_starting.ready");
    Ok(())
}

pub(crate) fn register_web_message_received(
    webview: &ICoreWebView2,
    inner: Arc<WebView2ProcessInner>,
) -> WebView2Result<()> {
    let msg_inner = inner.clone();
    let handler = WebMessageReceivedEventHandler::create(Box::new(move |_webview, args| {
        handle_web_message(msg_inner.clone(), args);
        Ok(())
    }));
    let mut token: i64 = 0;
    unsafe { webview.add_WebMessageReceived(&handler, &mut token) }.map_err(webview2_error)?;
    inner.metrics.mark("web_message.ready");
    Ok(())
}

pub(crate) fn install_bridge_script(
    webview: &ICoreWebView2,
    inner: &Arc<WebView2ProcessInner>,
) -> WebView2Result<()> {
    let commands = inner.command_allowlist.clone();
    let script =
        fenestra_bridge::install_script(&commands.iter().map(String::as_str).collect::<Vec<_>>());
    let wide = wide_pwstr(&script);
    let completed =
        AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(|_error, _id| Ok(())));
    unsafe { webview.AddScriptToExecuteOnDocumentCreated(PCWSTR(wide.as_ptr()), &completed) }
        .map_err(webview2_error)?;
    inner.metrics.mark("install_script.ready");
    Ok(())
}

fn handle_navigation_starting(
    inner: Arc<WebView2ProcessInner>,
    webview: Option<ICoreWebView2>,
    args: Option<ICoreWebView2NavigationStartingEventArgs>,
) {
    let (Some(webview), Some(args)) = (webview, args) else {
        return;
    };
    let url = match read_pwstr(|out| unsafe { args.Uri(out) }) {
        Some(value) => value,
        None => return,
    };
    if let Some(window_command) = WindowCommand::parse(&url) {
        let _ = unsafe { args.SetCancel(true) };
        let _ = dispatch_window_command(&inner, &window_command.action);
        return;
    }
    let Some(request) = parse_bridge_url(&url, None) else {
        return;
    };
    let _ = unsafe { args.SetCancel(true) };
    let allowed = inner
        .command_allowlist
        .iter()
        .any(|name| name == &request.command.name);
    if !allowed {
        resolve_bridge(
            &webview,
            &request.id,
            false,
            "{\"message\":\"bridge command not allowed\"}",
        );
        return;
    }
    let runtime = inner.bridge_runtime.lock().unwrap().clone();
    let Some(runtime) = runtime else {
        resolve_bridge(
            &webview,
            &request.id,
            false,
            "{\"message\":\"bridge runtime unavailable\"}",
        );
        return;
    };
    let response = runtime.dispatch(request.command);
    emit_bridge_response(&webview, &request.id, response);
}

fn handle_web_message(
    inner: Arc<WebView2ProcessInner>,
    args: Option<ICoreWebView2WebMessageReceivedEventArgs>,
) {
    let Some(args) = args else {
        return;
    };
    let mut raw = PWSTR(std::ptr::null_mut());
    let _ = unsafe { args.TryGetWebMessageAsString(&mut raw) };
    let message = if raw.0.is_null() {
        let _ = unsafe { args.WebMessageAsJson(&mut raw) };
        if raw.0.is_null() {
            return;
        }
        webview2_com::string_from_pcwstr(&PCWSTR(raw.0))
    } else {
        webview2_com::string_from_pcwstr(&PCWSTR(raw.0))
    };
    if let Some(window_command) = WindowCommand::parse(message.trim_matches('"')) {
        let _ = dispatch_window_command(&inner, &window_command.action);
    }
}

fn dispatch_window_command(inner: &Arc<WebView2ProcessInner>, action: &str) -> bool {
    let event = match action {
        "show" => WebView2UserEvent::Show,
        "hide" => WebView2UserEvent::Hide,
        "focus" => WebView2UserEvent::Focus,
        "minimize" => WebView2UserEvent::Minimize,
        "maximize" => WebView2UserEvent::Maximize,
        "unmaximize" => WebView2UserEvent::Unmaximize,
        "close" => WebView2UserEvent::Exit,
        _ => return false,
    };
    event.dispatch(&inner.event_sender)
}

fn emit_bridge_response(webview: &ICoreWebView2, id: &str, response: BridgeResult) {
    let payload = bridge_response_payload(&response);
    let script = format!(
        "window.__fenestraBridgeResolve&&window.__fenestraBridgeResolve({},{},{});",
        json_string(id),
        if response.is_ok() { "true" } else { "false" },
        payload,
    );
    execute_script(webview, &script);
}

fn resolve_bridge(webview: &ICoreWebView2, id: &str, ok: bool, payload_json: &str) {
    let script = format!(
        "window.__fenestraBridgeResolve&&window.__fenestraBridgeResolve({},{},{});",
        json_string(id),
        if ok { "true" } else { "false" },
        payload_json,
    );
    execute_script(webview, &script);
}

pub(crate) fn execute_script(webview: &ICoreWebView2, script: &str) {
    let wide = wide_pwstr(script);
    let handler = ExecuteScriptCompletedHandler::create(Box::new(|_error, _result| Ok(())));
    let _ = unsafe { webview.ExecuteScript(PCWSTR(wide.as_ptr()), &handler) };
}

pub(crate) fn execute_bridge_emit(
    webview: &ICoreWebView2,
    name: &str,
    payload: &serde_json::Value,
) {
    let name_json = json_string(name);
    let payload_json = match serde_json::to_string(payload) {
        Ok(value) => value,
        Err(_) => "null".to_string(),
    };
    let script = format!(
        "window.__fenestraBridgeEmit&&window.__fenestraBridgeEmit({name_json},{payload_json});"
    );
    execute_script(webview, &script);
}

fn bridge_response_payload(response: &BridgeResult) -> String {
    match response {
        Ok(payload) => {
            serde_json::to_string(&payload.result).unwrap_or_else(|_| "null".to_string())
        }
        Err(error) => serde_json::to_string(&serde_json::json!({ "message": error.message }))
            .unwrap_or_else(|_| "null".to_string()),
    }
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

pub(crate) fn wide_pwstr(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn webview2_error(error: windows::core::Error) -> WebView2Error {
    WebView2Error::Backend(format!("WebView2: {error}"))
}

fn read_pwstr<F: FnOnce(*mut PWSTR) -> windows::core::Result<()>>(read: F) -> Option<String> {
    let mut raw = PWSTR(std::ptr::null_mut());
    read(&mut raw).ok()?;
    if raw.0.is_null() {
        return None;
    }
    Some(webview2_com::string_from_pcwstr(&PCWSTR(raw.0)))
}
