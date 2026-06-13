#include "app.h"

#if defined(OS_WIN) || defined(_WIN32)
#include <windows.h>
#endif

#if defined(CEF_X11)
#include <X11/Xlib.h>
#endif

#include <string>

#include "include/base/cef_compiler_specific.h"
#include "include/cef_app.h"
#include "include/cef_command_line.h"

#if defined(CEF_X11)
namespace {
int XErrorHandlerImpl(Display* display, XErrorEvent* event) {
  return 0;
}

int XIOErrorHandlerImpl(Display* display) {
  return 0;
}
}  // namespace
#endif

namespace {

int RunFenestraHost(CefMainArgs main_args, int argc, char* argv[]) {
  CefRefPtr<FenestraApp> app(new FenestraApp);

  int exit_code = CefExecuteProcess(main_args, app.get(), nullptr);
  if (exit_code >= 0) {
    return exit_code;
  }

#if defined(CEF_X11)
  XSetErrorHandler(XErrorHandlerImpl);
  XSetIOErrorHandler(XIOErrorHandlerImpl);
#endif

  CefRefPtr<CefCommandLine> command_line = CefCommandLine::CreateCommandLine();
  command_line->InitFromArgv(argc, argv);

  CefSettings settings;
  settings.no_sandbox = true;
  if (command_line->HasSwitch("fenestra-osr")) {
    settings.windowless_rendering_enabled = true;
  }

  const std::string root_cache_path =
      command_line->GetSwitchValue("root-cache-path");
  if (!root_cache_path.empty()) {
    CefString(&settings.root_cache_path).FromString(root_cache_path);
  }

  const std::string cache_path = command_line->GetSwitchValue("cache-path");
  if (!cache_path.empty()) {
    CefString(&settings.cache_path).FromString(cache_path);
  }

  if (!CefInitialize(main_args, settings, app.get(), nullptr)) {
    return CefGetExitCode();
  }

  CefRunMessageLoop();
  CefShutdown();
  return 0;
}

}  // namespace

#if defined(OS_WIN) || defined(_WIN32)
int APIENTRY wWinMain(HINSTANCE hInstance,
                      HINSTANCE hPrevInstance,
                      LPWSTR lpCmdLine,
                      int nCmdShow) {
  (void)hPrevInstance;
  (void)lpCmdLine;
  (void)nCmdShow;
  CefMainArgs main_args(hInstance);
  return RunFenestraHost(main_args, __argc, __argv);
}
#else
NO_STACK_PROTECTOR
int main(int argc, char* argv[]) {
  CefMainArgs main_args(argc, argv);
  return RunFenestraHost(main_args, argc, argv);
}
#endif
