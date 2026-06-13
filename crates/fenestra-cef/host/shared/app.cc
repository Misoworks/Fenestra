#include "app.h"

#include <algorithm>
#include <cstdlib>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

#include "handler.h"
#include "include/cef_browser.h"
#include "include/cef_command_line.h"
#include "include/views/cef_browser_view.h"
#include "include/views/cef_window.h"
#include "include/wrapper/cef_helpers.h"
#include "osr_handler.h"

namespace {

int SwitchInt(CefRefPtr<CefCommandLine> command_line,
              const std::string& name,
              int fallback) {
  const std::string value = command_line->GetSwitchValue(name);
  if (value.empty()) {
    return fallback;
  }
  return std::atoi(value.c_str());
}

CefWindowHandle ParentWindow(CefRefPtr<CefCommandLine> command_line) {
  const std::string value = command_line->GetSwitchValue("fenestra-parent-window");
  if (value.empty()) {
    return kNullWindowHandle;
  }
  return static_cast<CefWindowHandle>(std::strtoull(value.c_str(), nullptr, 0));
}

std::vector<std::string> BridgeCommands(CefRefPtr<CefCommandLine> command_line) {
  std::vector<std::string> commands;
  std::stringstream stream(
      std::string(command_line->GetSwitchValue("fenestra-bridge-commands")));
  std::string item;
  while (std::getline(stream, item, ',')) {
    if (!item.empty()) {
      commands.push_back(item);
    }
  }
  return commands;
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

std::string JsArray(const std::vector<std::string>& values) {
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

std::string BridgeInstallScript(const std::vector<std::string>& commands) {
  return "(function(){"
         "if(window.fenestra&&window.fenestra.bridge&&window.fenestra.bridge.__native)return;"
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
         "window.fenestra=window.fenestra||{};"
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

class FenestraBrowserViewDelegate : public CefBrowserViewDelegate {
 public:
  CefRefPtr<CefBrowserViewDelegate> GetDelegateForPopupBrowserView(
      CefRefPtr<CefBrowserView> browser_view,
      const CefBrowserSettings& settings,
      CefRefPtr<CefClient> client,
      bool is_devtools) override {
    return this;
  }

  cef_runtime_style_t GetBrowserRuntimeStyle() override {
    return CEF_RUNTIME_STYLE_ALLOY;
  }

 private:
  IMPLEMENT_REFCOUNTING(FenestraBrowserViewDelegate);
};

class FenestraWindowDelegate : public CefWindowDelegate {
 public:
	  FenestraWindowDelegate(CefRefPtr<CefBrowserView> browser_view,
	                     std::string title,
	                     int width,
	                     int height,
	                     bool frameless,
	                     bool visible)
	      : browser_view_(browser_view),
	        title_(std::move(title)),
	        width_(width),
	        height_(height),
	        frameless_(frameless),
	        visible_(visible) {}

  void OnWindowCreated(CefRefPtr<CefWindow> window) override {
	    window->SetTitle(title_);
	    window->AddChildView(browser_view_);
	    window->CenterWindow(CefSize(width_, height_));
	    if (visible_) {
	      window->Show();
	    }
  }

  void OnWindowDestroyed(CefRefPtr<CefWindow> window) override {
    browser_view_ = nullptr;
  }

  bool CanClose(CefRefPtr<CefWindow> window) override {
    CefRefPtr<CefBrowser> browser = browser_view_ ? browser_view_->GetBrowser() : nullptr;
    return browser ? browser->GetHost()->TryCloseBrowser() : true;
  }

  CefSize GetPreferredSize(CefRefPtr<CefView> view) override {
    return CefSize(width_, height_);
  }

  CefRect GetInitialBounds(CefRefPtr<CefWindow> window) override {
    return frameless_ ? CefRect(0, 0, width_, height_) : CefRect();
  }

  bool IsFrameless(CefRefPtr<CefWindow> window) override {
    return frameless_;
  }

  cef_runtime_style_t GetWindowRuntimeStyle() override {
    return CEF_RUNTIME_STYLE_ALLOY;
  }

 private:
  CefRefPtr<CefBrowserView> browser_view_;
  const std::string title_;
	  const int width_;
	  const int height_;
	  const bool frameless_;
	  const bool visible_;

  IMPLEMENT_REFCOUNTING(FenestraWindowDelegate);
};

void CreateFenestraBrowserWindow(CefRefPtr<CefCommandLine> command_line) {
  if (command_line->HasSwitch("fenestra-osr")) {
    CreateFenestraOsrBrowser(command_line);
    return;
  }

  const std::string url_value = command_line->GetSwitchValue("url");
  const std::string url =
      url_value.empty() ? "about:blank" : std::string(url_value);
  const std::string title_value = command_line->GetSwitchValue("fenestra-title");
  const std::string title =
      title_value.empty() ? "Fenestra" : std::string(title_value);
  const int x = SwitchInt(command_line, "fenestra-x", 0);
  const int y = SwitchInt(command_line, "fenestra-y", 0);
	  const int width = std::max(1, SwitchInt(command_line, "fenestra-width", 800));
	  const int height = std::max(1, SwitchInt(command_line, "fenestra-height", 600));
	  const bool frameless = command_line->HasSwitch("fenestra-frameless");
	  const bool visible = !command_line->HasSwitch("fenestra-hidden");

  CefBrowserSettings browser_settings;
  if (command_line->HasSwitch("fenestra-transparent")) {
    browser_settings.background_color = CefColorSetARGB(0, 0, 0, 0);
  }
  CefWindowInfo window_info;
  window_info.runtime_style = CEF_RUNTIME_STYLE_ALLOY;

  CefWindowHandle parent = ParentWindow(command_line);
  CefRefPtr<FenestraHandler> handler(new FenestraHandler(
      BridgeCommands(command_line), command_line->HasSwitch("fenestra-transparent")));
  if (parent != kNullWindowHandle) {
    window_info.SetAsChild(parent, CefRect(x, y, width, height));
    CefBrowserHost::CreateBrowser(window_info, handler, url, browser_settings,
                                  nullptr, nullptr);
  } else if (!frameless) {
    CefString(&window_info.window_name) = title;
    window_info.bounds = CefRect(x, y, width, height);
    CefBrowserHost::CreateBrowser(window_info, handler, url, browser_settings,
                                  nullptr, nullptr);
	  } else {
	    CefRefPtr<CefBrowserView> browser_view = CefBrowserView::CreateBrowserView(
	        handler, url, browser_settings, nullptr, nullptr,
	        new FenestraBrowserViewDelegate());
	    CefWindow::CreateTopLevelWindow(
	        new FenestraWindowDelegate(browser_view, title, width, height, frameless, visible));
	  }
}

}  // namespace

FenestraApp::FenestraApp() = default;

void FenestraApp::OnBeforeCommandLineProcessing(
    const CefString& process_type,
    CefRefPtr<CefCommandLine> command_line) {
  command_line->AppendSwitch("disable-vulkan");
  const std::string ozone_platform =
      command_line->GetSwitchValue("fenestra-ozone-platform");
  if (!ozone_platform.empty()) {
    command_line->AppendSwitchWithValue("ozone-platform", ozone_platform);
    command_line->AppendSwitchWithValue("ozone-platform-hint", ozone_platform);
  }
  command_line->AppendSwitchWithValue(
      "disable-features",
      "Vulkan,DefaultANGLEVulkan,VulkanFromANGLE,OptimizationGuideOnDeviceModel");
  command_line->AppendSwitch("disable-gpu");
  command_line->AppendSwitchWithValue("password-store", "basic");
  if (command_line->HasSwitch("fenestra-transparent")) {
    command_line->AppendSwitch("enable-transparent-visuals");
    command_line->AppendSwitch("transparent-painting-enabled");
    command_line->AppendSwitchWithValue("default-background-color", "0x00000000");
  }
}

void FenestraApp::OnContextInitialized() {
  CEF_REQUIRE_UI_THREAD();
  CefRefPtr<CefCommandLine> command_line =
      CefCommandLine::GetGlobalCommandLine();
  CreateFenestraBrowserWindow(command_line);
}

void FenestraApp::OnContextCreated(CefRefPtr<CefBrowser> browser,
                               CefRefPtr<CefFrame> frame,
                               CefRefPtr<CefV8Context> context) {
  CEF_REQUIRE_RENDERER_THREAD();
  if (!frame->IsMain()) {
    return;
  }
  CefRefPtr<CefCommandLine> command_line =
      CefCommandLine::GetGlobalCommandLine();
  const auto commands = BridgeCommands(command_line);
  if (commands.empty()) {
    return;
  }
  frame->ExecuteJavaScript(BridgeInstallScript(commands), frame->GetURL(), 0);
}

bool FenestraApp::OnAlreadyRunningAppRelaunch(
    CefRefPtr<CefCommandLine> command_line,
    const CefString& current_directory) {
  CEF_REQUIRE_UI_THREAD();
  CreateFenestraBrowserWindow(command_line);
  return true;
}

CefRefPtr<CefClient> FenestraApp::GetDefaultClient() {
  if (FenestraOsrHandler* handler = FenestraOsrHandler::GetInstance()) {
    return handler;
  }
  return FenestraHandler::GetInstance();
}
