#ifndef FENESTRA_CEF_HOST_OSR_HANDLER_H_
#define FENESTRA_CEF_HOST_OSR_HANDLER_H_

#include <list>
#include <mutex>
#include <set>
#include <string>
#include <vector>

#include "include/cef_client.h"
#include "include/cef_command_line.h"
#include "include/cef_context_menu_handler.h"
#include "include/cef_display_handler.h"
#include "include/cef_render_handler.h"

class FenestraOsrHandler : public CefClient,
                       public CefContextMenuHandler,
                       public CefDisplayHandler,
                       public CefLifeSpanHandler,
                       public CefLoadHandler,
                       public CefRenderHandler,
                       public CefRequestHandler {
 public:
  FenestraOsrHandler(std::string socket_path,
                 int width,
                 int height,
	                 float scale,
	                 std::vector<std::string> bridge_commands,
	                 bool transparent_background,
	                 int active_frame_rate,
	                 int background_frame_rate);
  ~FenestraOsrHandler() override;

  static FenestraOsrHandler* GetInstance();

  CefRefPtr<CefContextMenuHandler> GetContextMenuHandler() override { return this; }
  CefRefPtr<CefDisplayHandler> GetDisplayHandler() override { return this; }
  CefRefPtr<CefLifeSpanHandler> GetLifeSpanHandler() override { return this; }
  CefRefPtr<CefLoadHandler> GetLoadHandler() override { return this; }
  CefRefPtr<CefRenderHandler> GetRenderHandler() override { return this; }
  CefRefPtr<CefRequestHandler> GetRequestHandler() override { return this; }

  void OnBeforeContextMenu(CefRefPtr<CefBrowser> browser,
                           CefRefPtr<CefFrame> frame,
                           CefRefPtr<CefContextMenuParams> params,
                           CefRefPtr<CefMenuModel> model) override;
  bool OnContextMenuCommand(CefRefPtr<CefBrowser> browser,
                            CefRefPtr<CefFrame> frame,
                            CefRefPtr<CefContextMenuParams> params,
                            int command_id,
                            EventFlags event_flags) override;
  bool OnCursorChange(CefRefPtr<CefBrowser> browser,
                      CefCursorHandle cursor,
                      cef_cursor_type_t type,
                      const CefCursorInfo& custom_cursor_info) override;
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

  bool GetScreenInfo(CefRefPtr<CefBrowser> browser,
                     CefScreenInfo& screen_info) override;
  void GetViewRect(CefRefPtr<CefBrowser> browser, CefRect& rect) override;
  void OnPopupShow(CefRefPtr<CefBrowser> browser, bool show) override;
  void OnPopupSize(CefRefPtr<CefBrowser> browser, const CefRect& rect) override;
  void OnPaint(CefRefPtr<CefBrowser> browser,
               PaintElementType type,
               const RectList& dirtyRects,
               const void* buffer,
               int width,
               int height) override;
  bool StartDragging(CefRefPtr<CefBrowser> browser,
                     CefRefPtr<CefDragData> drag_data,
                     DragOperationsMask allowed_ops,
                     int x,
                     int y) override;
  void UpdateDragCursor(CefRefPtr<CefBrowser> browser,
                        DragOperation operation) override;

  void HandleControlLine(const std::string& line);
  void ResolveBridgeResponse(const std::string& browser_id,
                             const std::string& request_id,
                             bool ok,
                             const std::string& payload);
  void EmitBridgeEvent(const std::string& name_json,
                       const std::string& payload);

 private:
  using BrowserList = std::list<CefRefPtr<CefBrowser>>;

  bool ConnectSocket();
  bool SendMessage(uint32_t kind,
                   uint32_t width,
                   uint32_t height,
                   int32_t x,
                   int32_t y,
                   const void* payload,
                   uint32_t payload_len);
  bool SendMessageWithFd(uint32_t kind,
                         uint32_t width,
                         uint32_t height,
                         int32_t x,
                         int32_t y,
                         const void* payload,
                         uint32_t payload_len,
                         int fd);
  bool SendPaintBatch(uint32_t kind,
                      int32_t origin_x,
                      int32_t origin_y,
                      const void* buffer,
                      int buffer_width,
                      int buffer_height,
                      const RectList& dirty_rects);
  bool HandleBridgeCommand(CefRefPtr<CefBrowser> browser,
                           CefRefPtr<CefFrame> frame,
                           const std::string& url);
  bool HandleWindowCommand(CefRefPtr<CefBrowser> browser, const std::string& url);
  void RequestNativeClose();
	  void InstallBridge(CefRefPtr<CefBrowser> browser, CefRefPtr<CefFrame> frame);
	  void InstallTransparentBackground(CefRefPtr<CefFrame> frame);
	  void ApplyLifecycle(const std::string& state, int frame_rate, const std::string& reason);
	  void DispatchLifecycle(const std::string& state, const std::string& reason);
	  void StartCommandReader();

  BrowserList browsers_;
  CefRefPtr<CefBrowser> browser_;
  std::string socket_path_;
  int socket_fd_ = -1;
  std::mutex socket_mutex_;
  int width_ = 1;
  int height_ = 1;
  float scale_ = 1.0f;
  CefRect popup_rect_;
	  std::set<std::string> bridge_commands_;
	  bool transparent_background_ = false;
	  bool suspended_ = false;
	  int active_frame_rate_ = 60;
	  int background_frame_rate_ = 5;
	  bool closing_ = false;

  IMPLEMENT_REFCOUNTING(FenestraOsrHandler);
};

void CreateFenestraOsrBrowser(CefRefPtr<CefCommandLine> command_line);

#endif
