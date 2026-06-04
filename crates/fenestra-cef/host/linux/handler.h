#ifndef FENESTRA_CEF_HOST_HANDLER_H_
#define FENESTRA_CEF_HOST_HANDLER_H_

#include <list>
#include <set>
#include <string>
#include <vector>

#include "include/cef_client.h"

class FenestraHandler : public CefClient,
                    public CefDragHandler,
                    public CefDisplayHandler,
                    public CefLifeSpanHandler,
                    public CefLoadHandler,
                    public CefRequestHandler {
 public:
  FenestraHandler(std::vector<std::string> bridge_commands,
              bool transparent_background);
  ~FenestraHandler() override;

  static FenestraHandler* GetInstance();

  CefRefPtr<CefDragHandler> GetDragHandler() override { return this; }
  CefRefPtr<CefDisplayHandler> GetDisplayHandler() override { return this; }
  CefRefPtr<CefLifeSpanHandler> GetLifeSpanHandler() override { return this; }
  CefRefPtr<CefLoadHandler> GetLoadHandler() override { return this; }
  CefRefPtr<CefRequestHandler> GetRequestHandler() override { return this; }

  void OnDraggableRegionsChanged(
      CefRefPtr<CefBrowser> browser,
      CefRefPtr<CefFrame> frame,
      const std::vector<CefDraggableRegion>& regions) override;
  void OnTitleChange(CefRefPtr<CefBrowser> browser,
                     const CefString& title) override;
  void OnAfterCreated(CefRefPtr<CefBrowser> browser) override;
  bool DoClose(CefRefPtr<CefBrowser> browser) override;
  void OnBeforeClose(CefRefPtr<CefBrowser> browser) override;
  void OnLoadError(CefRefPtr<CefBrowser> browser,
                   CefRefPtr<CefFrame> frame,
                   ErrorCode errorCode,
                   const CefString& errorText,
                   const CefString& failedUrl) override;
  void OnLoadStart(CefRefPtr<CefBrowser> browser,
                   CefRefPtr<CefFrame> frame,
                   TransitionType transition_type) override;
  void OnLoadEnd(CefRefPtr<CefBrowser> browser,
                 CefRefPtr<CefFrame> frame,
                 int httpStatusCode) override;
  bool OnBeforeBrowse(CefRefPtr<CefBrowser> browser,
                      CefRefPtr<CefFrame> frame,
                      CefRefPtr<CefRequest> request,
                      bool user_gesture,
                      bool is_redirect) override;
  void ResolveBridgeResponse(const std::string& browser_id,
                             const std::string& request_id,
                             bool ok,
                             const std::string& payload);
	  void EmitBridgeEvent(const std::string& name_json,
	                       const std::string& payload);
	  void ApplyHostControl(const std::string& command, const std::string& value);

 private:
  using BrowserList = std::list<CefRefPtr<CefBrowser>>;
  bool HandleBridgeCommand(CefRefPtr<CefBrowser> browser,
                           CefRefPtr<CefFrame> frame,
                           const std::string& url);
  void InstallBridge(CefRefPtr<CefBrowser> browser,
                     CefRefPtr<CefFrame> frame);
  void InstallTransparentBackground(CefRefPtr<CefFrame> frame);

  BrowserList browsers_;
  std::set<std::string> bridge_commands_;
  bool transparent_background_ = false;
  bool closing_ = false;

  IMPLEMENT_REFCOUNTING(FenestraHandler);
};

#endif
