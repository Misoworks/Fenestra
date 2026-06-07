#include "handler.h"

#include <atomic>
#include <cstdlib>
#include <iostream>
#include <sstream>
#include <string>
#include <thread>
#include <utility>
#include <vector>

#include "include/cef_app.h"
#include "include/cef_parser.h"
#include "include/cef_task.h"
#include "include/views/cef_browser_view.h"
#include "include/views/cef_window.h"
#include "include/wrapper/cef_helpers.h"

namespace {
FenestraHandler* g_instance = nullptr;
std::atomic<bool> g_bridge_reader_started{false};

class QuitMessageLoopTask : public CefTask {
 public:
  void Execute() override {
    CefQuitMessageLoop();
  }

 private:
  IMPLEMENT_REFCOUNTING(QuitMessageLoopTask);
};

void ScheduleQuitMessageLoopFallback() {
  CefPostDelayedTask(TID_UI, new QuitMessageLoopTask, 150);
}

std::string DataUri(const std::string& body) {
  return "data:text/html;base64," +
         CefURIEncode(CefBase64Encode(body.data(), body.size()), false)
             .ToString();
}

CefRefPtr<CefWindow> WindowForBrowser(CefRefPtr<CefBrowser> browser) {
  CefRefPtr<CefBrowserView> browser_view =
      CefBrowserView::GetForBrowser(browser);
  return browser_view ? browser_view->GetWindow() : nullptr;
}

std::string DecodeUriComponent(const std::string& value) {
  return CefURIDecode(
             value, true,
             static_cast<cef_uri_unescape_rule_t>(
                 UU_SPACES | UU_URL_SPECIAL_CHARS_EXCEPT_PATH_SEPARATORS |
                 UU_REPLACE_PLUS_WITH_SPACE))
      .ToString();
}

std::string QueryValue(const std::string& url, const std::string& name) {
  const size_t query_start = url.find('?');
  if (query_start == std::string::npos) {
    return "";
  }

  const std::string needle = name + "=";
  size_t cursor = query_start + 1;
  while (cursor < url.size()) {
    const size_t next = url.find('&', cursor);
    const size_t end = next == std::string::npos ? url.size() : next;
    const std::string part = url.substr(cursor, end - cursor);
    if (part.rfind(needle, 0) == 0) {
      return DecodeUriComponent(part.substr(needle.size()));
    }
    if (next == std::string::npos) {
      break;
    }
    cursor = next + 1;
  }
  return "";
}

std::string UrlOrigin(const std::string& url) {
  const size_t scheme_end = url.find("://");
  if (scheme_end == std::string::npos) {
    return "null";
  }

  const std::string scheme = url.substr(0, scheme_end);
  if (scheme == "file" || scheme == "about" || scheme == "devtools") {
    return scheme + "://";
  }

  const size_t authority_start = scheme_end + 3;
  const size_t authority_end = url.find_first_of("/?#", authority_start);
  const std::string authority = url.substr(
      authority_start,
      authority_end == std::string::npos ? std::string::npos
                                         : authority_end - authority_start);
  if (authority.empty()) {
    return "null";
  }
  return scheme + "://" + authority;
}

std::string BridgeRequestId(const std::string& url) {
  const std::string prefix = "fenestra://bridge/";
  if (url.rfind(prefix, 0) != 0) {
    return "";
  }
  const size_t start = prefix.size();
  const size_t end = url.find_first_of("?#", start);
  return DecodeUriComponent(
      url.substr(start, end == std::string::npos ? std::string::npos : end - start));
}

std::string JsString(const std::string& value) {
  std::string output = "\"";
  for (char c : value) {
    switch (c) {
      case '\\':
        output += "\\\\";
        break;
      case '"':
        output += "\\\"";
        break;
      case '\n':
        output += "\\n";
        break;
      case '\r':
        output += "\\r";
        break;
      case '\t':
        output += "\\t";
        break;
      default:
        output += c;
        break;
    }
  }
  output += "\"";
  return output;
}

std::string JsArray(const std::set<std::string>& values) {
  std::string output = "[";
  bool first = true;
  for (const auto& value : values) {
    if (!first) {
      output += ",";
    }
    output += JsString(value);
    first = false;
  }
  output += "]";
  return output;
}

std::string BridgeInstallScript(const std::set<std::string>& commands) {
  return "(function(){"
         "window.fenestra=window.fenestra||{};"
         "if(window.fenestra.__nativeApiVersion===1)return;"
         "window.fenestra.__nativeApiVersion=1;"
         "const commands=new Set(" +
         JsArray(commands) +
         ");"
         "const pending=new Map();const listeners=new Map();let nextId=1;"
         "window.__fenestraBridgeResolve=function(id,ok,payload){"
         "const entry=pending.get(String(id));if(!entry)return;"
         "pending.delete(String(id));"
         "if(ok){entry.resolve(payload);}else{entry.reject(new Error((payload&&payload.message)||'Fenestra bridge command failed'));}"
         "};"
         "window.__fenestraBridgeEmit=function(name,payload){"
         "const set=listeners.get(String(name));if(set){for(const cb of Array.from(set)){queueMicrotask(()=>cb(payload));}}"
         "window.dispatchEvent(new CustomEvent('fenestra:'+String(name),{detail:payload}));"
         "};"
         "const windowCommand=function(action){window.location.href='fenestra://window/'+action+'?at='+Date.now()+'-'+Math.random();};"
         "window.fenestra.window=Object.assign(window.fenestra.window||{},"
         "{show(){windowCommand('show');},hide(){windowCommand('hide');},focus(){windowCommand('focus');},"
         "close(){windowCommand('close');},minimize(){windowCommand('minimize');},"
         "maximize(){windowCommand('maximize');},toggleMaximize(){windowCommand('toggle-maximize');},"
         "restore(){windowCommand('restore');}});"
         "window.fenestra.bridge={__native:true,commands:Array.from(commands),listen(name,callback){"
         "const key=String(name);let set=listeners.get(key);if(!set){set=new Set();listeners.set(key,set);}set.add(callback);"
         "return()=>{set.delete(callback);if(!set.size)listeners.delete(key);};},invoke(name,params={}){"
         "if(!commands.has(name))return Promise.reject(new Error('Fenestra bridge command not registered: '+name));"
         "const id=String(nextId++);"
         "const payload=encodeURIComponent(JSON.stringify(params));"
         "const url='fenestra://bridge/'+encodeURIComponent(id)+'?name='+encodeURIComponent(name)+'&payload='+payload;"
         "return new Promise((resolve,reject)=>{"
         "pending.set(id,{resolve,reject});"
         "setTimeout(()=>{if(pending.has(id)){pending.delete(id);reject(new Error('Fenestra bridge command timed out: '+name));}},60000);"
         "window.location.href=url;"
         "});"
         "}};"
         "window.fenestra.activity={begin(options={}){"
         "return window.fenestra.bridge.invoke('fenestra.activity.begin',options).then(record=>{"
         "let ended=false;return Object.assign({},record,{end(){if(ended)return Promise.resolve({id:record.id,ended:false});ended=true;return window.fenestra.bridge.invoke('fenestra.activity.end',{id:record.id});}});});},"
         "list(){return window.fenestra.bridge.invoke('fenestra.activity.list');}};"
         "})();";
}

bool ParseBridgeResponse(const std::string& line,
                         std::string* browser_id,
                         std::string* request_id,
                         bool* ok,
                         std::string* payload) {
  const std::string prefix = "FENESTRA_BRIDGE_RESPONSE\t";
  if (line.rfind(prefix, 0) != 0) {
    return false;
  }
  std::vector<std::string> parts;
  size_t cursor = prefix.size();
  while (parts.size() < 3) {
    const size_t next = line.find('\t', cursor);
    if (next == std::string::npos) {
      return false;
    }
    parts.push_back(line.substr(cursor, next - cursor));
    cursor = next + 1;
  }
  *browser_id = parts[0];
  *request_id = parts[1];
  *ok = parts[2] == "ok";
  *payload = line.substr(cursor);
  return true;
}

bool ParseBridgeEvent(const std::string& line,
                      std::string* name_json,
                      std::string* payload) {
  const std::string prefix = "FENESTRA_BRIDGE_EVENT\t";
  if (line.rfind(prefix, 0) != 0) {
    return false;
  }
  const size_t separator = line.find('\t', prefix.size());
  if (separator == std::string::npos) {
    return false;
  }
  *name_json = line.substr(prefix.size(), separator - prefix.size());
  *payload = line.substr(separator + 1);
  return true;
}

bool ParseHostControl(const std::string& line,
                      std::string* command,
                      std::string* value) {
  const std::string prefix = "FENESTRA_HOST_CONTROL\t";
  if (line.rfind(prefix, 0) != 0) {
    return false;
  }
  const size_t separator = line.find('\t', prefix.size());
  if (separator == std::string::npos) {
    *command = line.substr(prefix.size());
    *value = "1";
    return !command->empty();
  }
  *command = line.substr(prefix.size(), separator - prefix.size());
  *value = line.substr(separator + 1);
  return !command->empty();
}

class BridgeResponseTask : public CefTask {
 public:
  BridgeResponseTask(std::string browser_id,
                     std::string request_id,
                     bool ok,
                     std::string payload)
      : browser_id_(std::move(browser_id)),
        request_id_(std::move(request_id)),
        ok_(ok),
        payload_(std::move(payload)) {}

  void Execute() override {
    if (FenestraHandler* handler = FenestraHandler::GetInstance()) {
      handler->ResolveBridgeResponse(browser_id_, request_id_, ok_, payload_);
    }
  }

 private:
  const std::string browser_id_;
  const std::string request_id_;
  const bool ok_;
  const std::string payload_;

  IMPLEMENT_REFCOUNTING(BridgeResponseTask);
};

class BridgeEventTask : public CefTask {
 public:
  BridgeEventTask(std::string name_json, std::string payload)
      : name_json_(std::move(name_json)), payload_(std::move(payload)) {}

  void Execute() override {
    if (FenestraHandler* handler = FenestraHandler::GetInstance()) {
      handler->EmitBridgeEvent(name_json_, payload_);
    }
  }

 private:
  const std::string name_json_;
  const std::string payload_;

  IMPLEMENT_REFCOUNTING(BridgeEventTask);
};

class HostControlTask : public CefTask {
 public:
  HostControlTask(std::string command, std::string value)
      : command_(std::move(command)), value_(std::move(value)) {}

  void Execute() override {
    if (auto* handler = FenestraHandler::GetInstance()) {
      handler->ApplyHostControl(command_, value_);
    }
  }

 private:
  std::string command_;
  std::string value_;

  IMPLEMENT_REFCOUNTING(HostControlTask);
};

void StartBridgeReader() {
  bool expected = false;
  if (!g_bridge_reader_started.compare_exchange_strong(expected, true)) {
    return;
  }
  std::thread([] {
    std::string line;
    while (std::getline(std::cin, line)) {
      std::string browser_id;
      std::string request_id;
      std::string name_json;
      std::string payload;
	      bool ok = false;
	      std::string command;
	      std::string value;
	      if (ParseHostControl(line, &command, &value)) {
	        CefPostTask(TID_UI, new HostControlTask(command, value));
	        continue;
	      }
	      if (!ParseBridgeResponse(line, &browser_id, &request_id, &ok, &payload)) {
        if (ParseBridgeEvent(line, &name_json, &payload)) {
          CefPostTask(TID_UI, new BridgeEventTask(name_json, payload));
        }
        continue;
      }
      CefPostTask(TID_UI,
                  new BridgeResponseTask(browser_id, request_id, ok, payload));
    }
  }).detach();
}

bool HandleWindowCommand(CefRefPtr<CefBrowser> browser,
                         const std::string& url) {
  const std::string prefix = "fenestra://window/";
  if (url.rfind(prefix, 0) != 0) {
    return false;
  }

  std::string command = url.substr(prefix.size());
  const size_t query = command.find_first_of("?#");
  if (query != std::string::npos) {
    command = command.substr(0, query);
  }

  CefRefPtr<CefWindow> window = WindowForBrowser(browser);
  if (!window) {
    return true;
  }

	  if (command == "close") {
	    if (!window->IsClosed()) {
	      window->Hide();
	    }
	    browser->GetHost()->CloseBrowser(false);
	    ScheduleQuitMessageLoopFallback();
	  } else if (command == "show") {
	    window->Show();
	  } else if (command == "hide") {
	    window->Hide();
	  } else if (command == "focus") {
	    window->Show();
	    window->Activate();
	  } else if (command == "minimize") {
	    window->Minimize();
  } else if (command == "maximize") {
    window->Maximize();
  } else if (command == "restore") {
    window->Restore();
  } else if (command == "toggle-maximize") {
    if (window->IsMaximized()) {
      window->Restore();
    } else {
      window->Maximize();
    }
  }

	  return true;
	}

}  // namespace

FenestraHandler::FenestraHandler(std::vector<std::string> bridge_commands,
                         bool transparent_background)
    : bridge_commands_(bridge_commands.begin(), bridge_commands.end()),
      transparent_background_(transparent_background) {
  if (!g_instance) {
    g_instance = this;
  }
  StartBridgeReader();
}

FenestraHandler::~FenestraHandler() {
  if (g_instance == this) {
    g_instance = nullptr;
  }
}

FenestraHandler* FenestraHandler::GetInstance() {
  return g_instance;
}

void FenestraHandler::OnTitleChange(CefRefPtr<CefBrowser> browser,
                                const CefString& title) {}

void FenestraHandler::OnDraggableRegionsChanged(
    CefRefPtr<CefBrowser> browser,
    CefRefPtr<CefFrame> frame,
    const std::vector<CefDraggableRegion>& regions) {
  CEF_REQUIRE_UI_THREAD();
  CefRefPtr<CefWindow> window = WindowForBrowser(browser);
  if (window) {
    window->SetDraggableRegions(regions);
  }
}

void FenestraHandler::OnAfterCreated(CefRefPtr<CefBrowser> browser) {
  CEF_REQUIRE_UI_THREAD();
  browsers_.push_back(browser);
}

bool FenestraHandler::DoClose(CefRefPtr<CefBrowser> browser) {
  CEF_REQUIRE_UI_THREAD();
  if (browsers_.size() == 1) {
    closing_ = true;
  }
  return false;
}

void FenestraHandler::OnBeforeClose(CefRefPtr<CefBrowser> browser) {
  CEF_REQUIRE_UI_THREAD();
  for (auto it = browsers_.begin(); it != browsers_.end(); ++it) {
    if ((*it)->IsSame(browser)) {
      browsers_.erase(it);
      break;
    }
  }
  if (browsers_.empty()) {
    CefQuitMessageLoop();
  }
}

void FenestraHandler::ApplyHostControl(const std::string& command,
                                       const std::string& value) {
  CEF_REQUIRE_UI_THREAD();
  for (auto& browser : browsers_) {
    CefRefPtr<CefWindow> window = WindowForBrowser(browser);
    if (!window || window->IsClosed()) {
      continue;
    }
    if (command == "visible") {
      if (value == "1" || value == "true" || value == "yes" ||
          value == "show" || value == "visible") {
        window->Show();
      } else if (value == "0" || value == "false" || value == "no" ||
                 value == "hide" || value == "hidden") {
        window->Hide();
      }
    } else if (command == "show") {
      window->Show();
    } else if (command == "hide") {
      window->Hide();
    } else if (command == "focus") {
      window->Show();
      window->Activate();
    }
  }
}

void FenestraHandler::OnLoadError(CefRefPtr<CefBrowser> browser,
                              CefRefPtr<CefFrame> frame,
                              ErrorCode errorCode,
                              const CefString& errorText,
                              const CefString& failedUrl) {
  CEF_REQUIRE_UI_THREAD();
  if (errorCode == ERR_ABORTED) {
    return;
  }
  std::stringstream body;
  body << "<!doctype html><meta charset=\"utf-8\"><body style=\"margin:0;"
          "font:14px system-ui;background:#111;color:#eee;padding:24px\">"
       << "<h2>Failed to load</h2><p>" << std::string(failedUrl) << "</p><p>"
       << std::string(errorText) << "</p></body>";
  frame->LoadURL(DataUri(body.str()));
}

void FenestraHandler::OnLoadStart(CefRefPtr<CefBrowser> browser,
                              CefRefPtr<CefFrame> frame,
                              TransitionType transition_type) {
  CEF_REQUIRE_UI_THREAD();
  if (frame->IsMain()) {
    InstallTransparentBackground(frame);
    InstallBridge(browser, frame);
  }
}

void FenestraHandler::OnLoadEnd(CefRefPtr<CefBrowser> browser,
                            CefRefPtr<CefFrame> frame,
                            int httpStatusCode) {
  CEF_REQUIRE_UI_THREAD();
  if (frame->IsMain()) {
    InstallTransparentBackground(frame);
    InstallBridge(browser, frame);
  }
}

bool FenestraHandler::OnBeforeBrowse(CefRefPtr<CefBrowser> browser,
                                 CefRefPtr<CefFrame> frame,
                                 CefRefPtr<CefRequest> request,
                                 bool user_gesture,
                                 bool is_redirect) {
  CEF_REQUIRE_UI_THREAD();
  const std::string url = request->GetURL();
  return HandleWindowCommand(browser, url) || HandleBridgeCommand(browser, frame, url);
}

bool FenestraHandler::HandleBridgeCommand(CefRefPtr<CefBrowser> browser,
                                      CefRefPtr<CefFrame> frame,
                                      const std::string& url) {
  const std::string prefix = "fenestra://bridge/";
  if (url.rfind(prefix, 0) != 0) {
    return false;
  }

  const std::string request_id = BridgeRequestId(url);
  const std::string command = QueryValue(url, "name");
  const std::string payload = QueryValue(url, "payload");
  const std::string browser_id = std::to_string(browser->GetIdentifier());
  const std::string origin = UrlOrigin(frame ? std::string(frame->GetURL()) : "");
  if (request_id.empty() || command.empty()) {
    ResolveBridgeResponse(browser_id, request_id, false,
                          "{\"message\":\"Malformed Fenestra bridge request\"}");
    return true;
  }
  if (!bridge_commands_.contains(command)) {
    ResolveBridgeResponse(
        browser_id, request_id, false,
        "{\"message\":\"Fenestra bridge command is not allowlisted\"}");
    return true;
  }

  std::cout << "FENESTRA_BRIDGE_REQUEST\t" << browser_id << "\t" << request_id
            << "\t" << origin << "\t" << command << "\t"
            << (payload.empty() ? "{}" : payload)
            << std::endl;
  return true;
}

void FenestraHandler::InstallBridge(CefRefPtr<CefBrowser> browser,
	                                CefRefPtr<CefFrame> frame) {
  frame->ExecuteJavaScript(BridgeInstallScript(bridge_commands_), frame->GetURL(),
	                           0);
}

void FenestraHandler::InstallTransparentBackground(CefRefPtr<CefFrame> frame) {
  if (!transparent_background_) {
    return;
  }
  frame->ExecuteJavaScript(
      "(function(){"
      "if(document.documentElement){document.documentElement.style.background='transparent';}"
      "if(document.body){document.body.style.background='transparent';}"
      "if(!document.querySelector('style[data-fenestra-transparent-background]')){"
      "const style=document.createElement('style');"
      "style.setAttribute('data-fenestra-transparent-background','');"
      "style.textContent='html,body{background:transparent!important;}';"
      "document.head&&document.head.appendChild(style);"
      "}"
      "})();",
      frame->GetURL(), 0);
}

void FenestraHandler::ResolveBridgeResponse(const std::string& browser_id,
                                        const std::string& request_id,
                                        bool ok,
                                        const std::string& payload) {
  CEF_REQUIRE_UI_THREAD();
  CefRefPtr<CefBrowser> target;
  const int expected_id = std::atoi(browser_id.c_str());
  for (auto& browser : browsers_) {
    if (browser->GetIdentifier() == expected_id) {
      target = browser;
      break;
    }
  }
  if (!target || request_id.empty()) {
    return;
  }
  const std::string script =
      "window.__fenestraBridgeResolve&&window.__fenestraBridgeResolve(" +
      JsString(request_id) + "," + (ok ? "true" : "false") + "," +
      (payload.empty() ? "null" : payload) + ");";
  target->GetMainFrame()->ExecuteJavaScript(script, target->GetMainFrame()->GetURL(),
                                            0);
}

void FenestraHandler::EmitBridgeEvent(const std::string& name_json,
                                      const std::string& payload) {
  CEF_REQUIRE_UI_THREAD();
  const std::string script =
      "window.__fenestraBridgeEmit&&window.__fenestraBridgeEmit(" +
      name_json + "," + (payload.empty() ? "null" : payload) + ");";
  for (auto& browser : browsers_) {
    browser->GetMainFrame()->ExecuteJavaScript(
        script, browser->GetMainFrame()->GetURL(), 0);
  }
}
