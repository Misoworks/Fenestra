use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

pub fn new_app(name: &str, template: &str) -> ExitCode {
    if template != "notes" {
        eprintln!("unknown template `{template}`; available templates: notes");
        return ExitCode::from(1);
    }
    if !is_valid_name(name) {
        eprintln!("app name must contain only letters, numbers, hyphens, and underscores");
        return ExitCode::from(1);
    }

    let root = PathBuf::from(name);
    if root.exists() {
        eprintln!("{} already exists", root.display());
        return ExitCode::from(1);
    }

    let result = write_notes_template(&root, name);
    match result {
        Ok(()) => {
            println!("Created Fenestra app at {}", root.display());
            println!("Run it with: cargo run");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("failed to create app: {error}");
            ExitCode::from(1)
        }
    }
}

fn write_notes_template(root: &Path, name: &str) -> std::io::Result<()> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("ui"))?;
    fs::write(root.join("Cargo.toml"), cargo_toml(name))?;
    fs::write(root.join("src/main.rs"), main_rs())?;
    fs::write(root.join("ui/index.html"), index_html())?;
    fs::write(root.join("ui/styles.css"), styles_css())?;
    fs::write(root.join("ui/app.js"), app_js())?;
    Ok(())
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn cargo_toml(name: &str) -> String {
    let fenestra_path = fenestra_cef_path()
        .map(|path| format!("{{ path = \"{}\" }}", path.display()))
        .unwrap_or_else(|| "\"0.1\"".to_string());
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

[dependencies]
fenestra-cef = {fenestra_path}
serde_json = "1"
"#
    )
}

fn fenestra_cef_path() -> Option<PathBuf> {
    let cli_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = cli_dir.parent()?.parent()?;
    let path = root.join("crates/fenestra-cef");
    path.exists().then_some(path)
}

fn main_rs() -> &'static str {
    r#"use std::path::PathBuf;

use fenestra_cef::{
    BridgeCommandDescriptor, BridgeResponse, CefWindow, CefWindowControlAction, RuntimeConfig,
    RuntimeMode, WindowRegion, WindowRegionRect, run_fenestra_host_from_args,
};

const APP_TITLEBAR_HEIGHT: i32 = 38;
const SIDEBAR_WIDTH: i32 = 260;

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if run_fenestra_host_from_args(&args) {
        return;
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let runtime = RuntimeConfig {
        mode: RuntimeMode::SharedPreferred,
        allow_user_install: true,
        bundled_dir: Some(manifest_dir.clone()),
        ..RuntimeConfig::default()
    };

    let window = CefWindow::new()
        .title("Notes")
        .size(900, 640)
        .entry(format!(
            "{}?chrome=app",
            manifest_dir.join("ui/index.html").display()
        ))
        .frameless()
        .glass()
        .blur_region(WindowRegion::adaptive_titlebar_sidebar(
            SIDEBAR_WIDTH,
            APP_TITLEBAR_HEIGHT,
            14,
        ))
        .opaque_region(WindowRegion::adaptive_content_after_sidebar(
            SIDEBAR_WIDTH,
            APP_TITLEBAR_HEIGHT,
        ))
        .input_region(WindowRegion::adaptive_rounded_rect(14))
        .drag_region(WindowRegionRect::new(0, 0, i32::MAX, APP_TITLEBAR_HEIGHT))
        .control_region(
            CefWindowControlAction::Minimize,
            WindowRegionRect::new(-100, 7, 24, 24),
        )
        .control_region(
            CefWindowControlAction::Maximize,
            WindowRegionRect::new(-68, 7, 24, 24),
        )
        .control_region(
            CefWindowControlAction::Close,
            WindowRegionRect::new(-36, 7, 24, 24),
        )
        .runtime(runtime)
        .bridge_descriptor_handler(
            BridgeCommandDescriptor::new("notes.create").target("desktop"),
            |command| {
                Ok(BridgeResponse::json(serde_json::json!({
                    "ok": true,
                    "params": command.params
                })))
            },
        );

    match window.launch_or_install() {
        Ok(process) => {
            let _ = process.wait();
        }
        Err(error) => {
            eprintln!("failed to launch webview: {error}");
            std::process::exit(1);
        }
    }
}
"#
}

fn index_html() -> &'static str {
    r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Notes</title>
    <script>
      document.documentElement.dataset.chrome =
        new URLSearchParams(location.search).get("chrome") || "system";
    </script>
    <link rel="stylesheet" href="./styles.css" />
  </head>
  <body>
    <div class="app-window">
      <div class="web-titlebar" aria-label="Window controls">
        <div class="web-titlebar-title">Notes</div>
        <div class="window-controls">
          <button class="window-control minimize" aria-label="Minimize" tabindex="-1"></button>
          <button class="window-control maximize" aria-label="Maximize" tabindex="-1"></button>
          <button class="window-control close" aria-label="Close" tabindex="-1"></button>
        </div>
      </div>
      <main class="shell">
        <aside class="sidebar">
          <h1>Notes</h1>
          <button id="new-note">New</button>
          <nav id="note-list"></nav>
          <p id="note-count">0 notes</p>
        </aside>
        <section class="content">
          <header>
            <input id="note-title" aria-label="Title" />
            <button id="save-note">Save</button>
          </header>
          <textarea id="note-body" aria-label="Body"></textarea>
        </section>
      </main>
    </div>
    <script src="./app.js" defer></script>
  </body>
</html>
"#
}

fn styles_css() -> &'static str {
    r#":root {
  --window-radius: 14px;
  --titlebar-height: 38px;
  color-scheme: dark;
  font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  background: transparent;
  color: rgb(244 244 244);
}

* {
  box-sizing: border-box;
}

html,
body {
  width: 100%;
  height: 100%;
  margin: 0;
  overflow: hidden;
  background: transparent;
  -webkit-font-smoothing: antialiased;
}

button,
input,
textarea {
  border: 0;
  border-radius: 8px;
  font: inherit;
  outline: none;
}

button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-height: 38px;
  padding: 0 18px;
  background: rgb(216 216 216);
  color: rgb(34 34 34);
  cursor: pointer;
  transition:
    background-color 120ms cubic-bezier(0.2, 0, 0, 1),
    color 120ms cubic-bezier(0.2, 0, 0, 1);
}

button:hover {
  background: rgb(235 235 235);
}

input,
textarea {
  background: rgb(118 118 118);
  color: rgb(248 248 248);
  caret-color: rgb(248 248 248);
}

.app-window {
  display: grid;
  grid-template-rows: 1fr;
  width: 100%;
  height: 100%;
  overflow: hidden;
  background: transparent;
  border-radius: var(--window-radius);
  isolation: isolate;
}

:root[data-chrome="app"] .app-window {
  grid-template-rows: var(--titlebar-height) 1fr;
}

.web-titlebar {
  position: relative;
  display: none;
  background: rgb(34 34 34 / 34%);
  backdrop-filter: blur(20px);
  box-shadow: inset 0 -1px 0 rgb(255 255 255 / 12%);
  user-select: none;
}

:root[data-chrome="app"] .web-titlebar {
  display: block;
}

.web-titlebar-title {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  color: rgb(245 245 245);
  font-size: 14px;
  font-weight: 600;
  pointer-events: none;
}

.window-controls {
  position: absolute;
  top: 7px;
  right: 12px;
  display: flex;
  gap: 8px;
}

.window-control {
  position: relative;
  width: 24px;
  min-width: 24px;
  height: 24px;
  min-height: 24px;
  padding: 0;
  border-radius: 999px;
  background: rgb(255 255 255 / 12%);
  color: rgb(245 245 245);
}

.window-control:hover {
  background: rgb(255 255 255 / 18%);
}

.window-control::before,
.window-control::after {
  position: absolute;
  content: "";
  background: currentColor;
}

.window-control.minimize::before {
  left: 7px;
  top: 11px;
  width: 10px;
  height: 2px;
  border-radius: 999px;
}

.window-control.maximize::before {
  left: 7px;
  top: 7px;
  width: 10px;
  height: 10px;
  border: 1.5px solid currentColor;
  background: transparent;
}

.window-control.close::before,
.window-control.close::after {
  left: 7px;
  top: 11px;
  width: 10px;
  height: 2px;
  border-radius: 999px;
}

.window-control.close::before {
  transform: rotate(45deg);
}

.window-control.close::after {
  transform: rotate(-45deg);
}

.shell {
  display: grid;
  grid-template-columns: 260px 1fr;
  min-height: 0;
  background: transparent;
}

.sidebar {
  display: flex;
  flex-direction: column;
  min-width: 0;
  padding: 22px 18px;
  background: rgb(32 32 32 / 28%);
  backdrop-filter: blur(20px);
  box-shadow: inset -1px 0 0 rgb(255 255 255 / 18%);
}

h1 {
  margin: 0 0 16px;
  font-size: 22px;
  font-weight: 500;
}

nav {
  display: grid;
  gap: 8px;
  margin-top: 14px;
  padding-top: 10px;
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 18%);
}

nav button {
  display: inline-flex;
  align-items: center;
  width: 100%;
  justify-content: flex-start;
  min-height: 40px;
  padding: 0 16px;
  background: transparent;
  color: rgb(242 242 242);
  text-align: left;
}

nav button:hover,
nav button.active {
  background: rgb(210 210 210);
  color: rgb(46 46 46);
}

.content {
  display: grid;
  grid-template-rows: 40px 1fr;
  gap: 14px;
  min-width: 0;
  padding: 30px 34px 28px;
  background: rgb(18 18 18 / 98%);
  box-shadow: inset 1px 0 0 rgb(255 255 255 / 8%);
}

header {
  display: grid;
  grid-template-columns: minmax(180px, 1fr) auto;
  gap: 10px;
}

input,
textarea {
  width: 100%;
  padding: 0 16px;
}

textarea {
  height: 100%;
  min-height: 0;
  padding: 16px;
  resize: none;
  line-height: 1.45;
}

#note-count {
  margin: auto 0 0;
}

@media (max-width: 720px) {
  .shell {
    grid-template-columns: 1fr;
    grid-template-rows: auto 1fr;
  }

  .sidebar {
    box-shadow: inset 0 -1px 0 rgb(255 255 255 / 18%);
  }
}
"#
}

fn app_js() -> &'static str {
    r##"let notes = [
  { id: "one", title: "Product notes", body: "Keep the app monochrome, calm, and functional." },
  { id: "two", title: "Runtime checklist", body: "Use the shared Fenestra runtime." },
  { id: "three", title: "Design pass", body: "Keep editing fast enough to feel native." },
];

let selected = 0;

const list = document.querySelector("#note-list");
const count = document.querySelector("#note-count");
const title = document.querySelector("#note-title");
const body = document.querySelector("#note-body");

function current() {
  return notes[selected];
}

function render() {
  list.replaceChildren(
    ...notes.map((note, index) => {
      const button = document.createElement("button");
      button.textContent = note.title;
      button.className = index === selected ? "active" : "";
      button.addEventListener("click", () => {
        saveFields();
        selected = index;
        render();
      });
      return button;
    }),
  );
  count.textContent = `${notes.length} notes`;
  title.value = current().title;
  body.value = current().body;
}

function saveFields() {
  current().title = title.value || "Untitled";
  current().body = body.value;
}

document.querySelector("#new-note").addEventListener("click", async () => {
  saveFields();
  const bridge = window.fenestra?.bridge;
  const created = bridge?.commands?.includes("notes.create")
    ? await bridge.invoke("notes.create", { title: "Untitled" })
    : null;
  notes.push({ id: created?.id || crypto.randomUUID(), title: "Untitled", body: "" });
  selected = notes.length - 1;
  render();
  title.focus();
});

document.querySelector("#save-note").addEventListener("click", () => {
  saveFields();
  render();
});

render();
"##
}
