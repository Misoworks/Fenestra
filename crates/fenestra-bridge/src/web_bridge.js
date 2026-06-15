// Fenestra bridge script. This is the single source of truth for the
// `window.fenestra` JS surface that lives inside every Fenestra webview.
//
// The host (CEF or WebView2) injects this script into every main frame
// after load. The host is expected to set `window.__fenestraBridgeCommands`
// to a JSON array of allowed bridge command names BEFORE this script runs;
// the script copies that list into a `Set` used for `invoke()` validation.
//
// The host should also implement a navigation handler for the
// `fenestra://bridge/<id>?name=<name>&payload=<payload>` scheme used by
// `invoke()`. The host parses the URL, dispatches to its registered bridge
// handler, then calls `window.__fenestraBridgeResolve(id, ok, payload)` from
// the host side to resolve the promise returned by `invoke()`.
//
// The host injects bridge events by calling
// `window.__fenestraBridgeEmit(name, payload)` from the host side.
// `window.fenestra.window.*` calls navigate to `fenestra://window/<action>`,
// which the host interprets as a host control (show/hide/focus/close/etc.)
// rather than a bridge command.
//
// This file is included as a `&str` from Rust via `include_str!`, embedded
// into the C++ CEF host as a generated header at build time, and posted
// into the WebView2 host as a string from the Rust fenestra-webview2 crate.
// Do not duplicate the body elsewhere; always edit this file.

(function () {
  if (window.fenestra && window.fenestra.bridge && window.fenestra.bridge.__native) return;
  const commands = new Set(window.__fenestraBridgeCommands || []);
  const pending = new Map();
  const listeners = new Map();
  let nextId = 1;

  window.__fenestraBridgeResolve = function (id, ok, payload) {
    const entry = pending.get(String(id));
    if (!entry) return;
    pending.delete(String(id));
    if (ok) {
      entry.resolve(payload);
    } else {
      entry.reject(new Error((payload && payload.message) || "Fenestra bridge command failed"));
    }
  };

  window.__fenestraBridgeEmit = function (name, payload) {
    const set = listeners.get(String(name));
    if (set) {
      for (const cb of Array.from(set)) {
        queueMicrotask(() => cb(payload));
      }
    }
    window.dispatchEvent(new CustomEvent("fenestra:" + String(name), { detail: payload }));
  };

  const windowCommand = function (action) {
    window.location.href =
      "fenestra://window/" + action + "?at=" + Date.now() + "-" + Math.random();
  };

  window.fenestra = window.fenestra || {};
  window.fenestra.window = Object.assign(window.fenestra.window || {}, {
    show() { windowCommand("show"); },
    hide() { windowCommand("hide"); },
    focus() { windowCommand("focus"); },
    close() { windowCommand("close"); },
    minimize() { windowCommand("minimize"); },
    maximize() { windowCommand("maximize"); },
    toggleMaximize() { windowCommand("toggle-maximize"); },
    restore() { windowCommand("restore"); },
  });

  window.fenestra.bridge = {
    __native: true,
    commands: Array.from(commands),
    listen(name, callback) {
      const key = String(name);
      let set = listeners.get(key);
      if (!set) { set = new Set(); listeners.set(key, set); }
      set.add(callback);
      return () => {
        set.delete(callback);
        if (!set.size) listeners.delete(key);
      };
    },
    invoke(name, params = {}) {
      if (!commands.has(name)) {
        return Promise.reject(new Error("Fenestra bridge command not registered: " + name));
      }
      const id = String(nextId++);
      const payload = encodeURIComponent(JSON.stringify(params));
      const url =
        "fenestra://bridge/" +
        encodeURIComponent(id) +
        "?name=" + encodeURIComponent(name) +
        "&payload=" + payload;
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        setTimeout(() => {
          if (pending.has(id)) {
            pending.delete(id);
            reject(new Error("Fenestra bridge command timed out: " + name));
          }
        }, 60000);
        window.location.href = url;
      });
    },
  };

  window.fenestra.activity = {
    begin(options = {}) {
      return window.fenestra.bridge.invoke("fenestra.activity.begin", options).then((record) => {
        let ended = false;
        return Object.assign({}, record, {
          end() {
            if (ended) return Promise.resolve({ id: record.id, ended: false });
            ended = true;
            return window.fenestra.bridge.invoke("fenestra.activity.end", { id: record.id });
          },
        });
      });
    },
    list() { return window.fenestra.bridge.invoke("fenestra.activity.list"); },
  };
})();
