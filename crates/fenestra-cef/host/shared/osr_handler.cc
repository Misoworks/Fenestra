#include "osr_handler.h"

#include <algorithm>
#include <atomic>
#include <cctype>
#include <cerrno>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <limits>
#include <sstream>
#include <string>
#include <thread>
#include <utility>
#include <vector>

#include <sys/socket.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/un.h>
#include <sys/uio.h>
#include <unistd.h>

#include "fenestra_bridge_js.h"
#include "include/cef_app.h"
#include "include/cef_browser.h"
#include "include/cef_parser.h"
#include "include/cef_task.h"
#include "include/internal/cef_types.h"
#include "include/wrapper/cef_helpers.h"

namespace {
FenestraOsrHandler* g_instance = nullptr;
std::atomic<bool> g_bridge_reader_started{false};
constexpr uint32_t kMainFrame = 1;
constexpr uint32_t kPopupFrame = 2;
constexpr uint32_t kMainBatch = 12;
constexpr uint32_t kPopupBatch = 13;
constexpr uint32_t kMainSharedBatch = 14;
constexpr uint32_t kPopupSharedBatch = 15;
constexpr uint32_t kFileDragRequested = 16;
constexpr size_t kSharedPaintThreshold = 256 * 1024;
constexpr size_t kBatchEntryLen = 28;
#ifndef MFD_CLOEXEC
constexpr unsigned int MFD_CLOEXEC = 0x0001U;
#endif

struct PaintRectBytes {
  int x = 0;
  int y = 0;
  int width = 0;
  int height = 0;
  uint64_t offset = 0;
  uint32_t len = 0;
};

int SwitchInt(CefRefPtr<CefCommandLine> command_line,
              const std::string& name,
              int fallback) {
  const std::string value = command_line->GetSwitchValue(name);
  if (value.empty()) {
    return fallback;
  }
  return std::atoi(value.c_str());
}

float SwitchFloat(CefRefPtr<CefCommandLine> command_line,
                  const std::string& name,
                  float fallback) {
  const std::string value = command_line->GetSwitchValue(name);
  if (value.empty()) {
    return fallback;
  }
  return std::atof(value.c_str());
}

std::vector<std::string> Split(const std::string& value, char separator) {
  std::vector<std::string> parts;
  std::stringstream stream(value);
  std::string item;
  while (std::getline(stream, item, separator)) {
    parts.push_back(item);
  }
  return parts;
}

std::vector<std::string> BridgeCommands(CefRefPtr<CefCommandLine> command_line) {
  std::vector<std::string> commands;
  for (const auto& item :
       Split(std::string(command_line->GetSwitchValue("fenestra-bridge-commands")), ',')) {
    if (!item.empty()) {
      commands.push_back(item);
    }
  }
  return commands;
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
  return authority.empty() ? "null" : scheme + "://" + authority;
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
  // See the matching comment in handler.cc: the canonical bridge script is
  // embedded as FENESTRA_BRIDGE_JS_RAW by host.rs at C++ build time.
  std::string prelude =
      "window.__fenestraBridgeCommands=" + JsArray(commands) + ";";
  return prelude + FENESTRA_BRIDGE_JS_RAW;
}

std::string DataUri(const std::string& body) {
  return "data:text/html;base64," + CefBase64Encode(body.data(), body.size()).ToString();
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
    if (FenestraOsrHandler* handler = FenestraOsrHandler::GetInstance()) {
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
    if (FenestraOsrHandler* handler = FenestraOsrHandler::GetInstance()) {
      handler->EmitBridgeEvent(name_json_, payload_);
    }
  }

 private:
  const std::string name_json_;
  const std::string payload_;

  IMPLEMENT_REFCOUNTING(BridgeEventTask);
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
      if (ParseBridgeResponse(line, &browser_id, &request_id, &ok, &payload)) {
        CefPostTask(TID_UI,
                    new BridgeResponseTask(browser_id, request_id, ok, payload));
      } else if (ParseBridgeEvent(line, &name_json, &payload)) {
        CefPostTask(TID_UI, new BridgeEventTask(name_json, payload));
      }
    }
  }).detach();
}

class OsrCommandTask : public CefTask {
 public:
  OsrCommandTask(CefRefPtr<FenestraOsrHandler> handler, std::string line)
      : handler_(handler), line_(std::move(line)) {}

  void Execute() override {
    handler_->HandleControlLine(line_);
  }

 private:
  CefRefPtr<FenestraOsrHandler> handler_;
  const std::string line_;

  IMPLEMENT_REFCOUNTING(OsrCommandTask);
};

class QuitTask : public CefTask {
 public:
  void Execute() override {
    CefQuitMessageLoop();
  }

 private:
  IMPLEMENT_REFCOUNTING(QuitTask);
};

void PutU32(std::vector<char>* buffer, size_t offset, uint32_t value) {
  (*buffer)[offset + 0] = static_cast<char>(value & 0xff);
  (*buffer)[offset + 1] = static_cast<char>((value >> 8) & 0xff);
  (*buffer)[offset + 2] = static_cast<char>((value >> 16) & 0xff);
  (*buffer)[offset + 3] = static_cast<char>((value >> 24) & 0xff);
}

void PutI32(std::vector<char>* buffer, size_t offset, int32_t value) {
  PutU32(buffer, offset, static_cast<uint32_t>(value));
}

void PutU64(std::vector<char>* buffer, size_t offset, uint64_t value) {
  for (size_t i = 0; i < 8; ++i) {
    (*buffer)[offset + i] = static_cast<char>((value >> (i * 8)) & 0xff);
  }
}

bool SendAll(int fd, const char* bytes, size_t len) {
  size_t sent = 0;
  while (sent < len) {
    const ssize_t result = send(fd, bytes + sent, len - sent, MSG_NOSIGNAL);
    if (result <= 0) {
      return false;
    }
    sent += static_cast<size_t>(result);
  }
  return true;
}

int CreateMemfd(const char* name) {
#ifdef SYS_memfd_create
  return static_cast<int>(syscall(SYS_memfd_create, name, MFD_CLOEXEC));
#else
  errno = ENOSYS;
  return -1;
#endif
}

bool WriteAllAt(int fd, const char* bytes, size_t len, off_t offset) {
  size_t written = 0;
  while (written < len) {
    const ssize_t result = pwrite(fd, bytes + written, len - written,
                                  offset + static_cast<off_t>(written));
    if (result <= 0) {
      return false;
    }
    written += static_cast<size_t>(result);
  }
  return true;
}

void PutPaintEntry(std::vector<char>* payload,
                   size_t offset,
                   const PaintRectBytes& rect) {
  PutI32(payload, offset + 0, rect.x);
  PutI32(payload, offset + 4, rect.y);
  PutU32(payload, offset + 8, static_cast<uint32_t>(rect.width));
  PutU32(payload, offset + 12, static_cast<uint32_t>(rect.height));
  PutU64(payload, offset + 16, rect.offset);
  PutU32(payload, offset + 24, rect.len);
}

bool CopyPaintRect(char* destination,
                   const void* buffer,
                   int buffer_width,
                   const PaintRectBytes& rect) {
  const char* source = static_cast<const char*>(buffer);
  const int source_stride = buffer_width * 4;
  const int row_bytes = rect.width * 4;
  for (int row = 0; row < rect.height; ++row) {
    std::memcpy(destination + rect.offset + static_cast<size_t>(row * row_bytes),
                source + (rect.y + row) * source_stride + rect.x * 4,
                row_bytes);
  }
  return true;
}

bool WritePaintRect(int fd,
                    const void* buffer,
                    int buffer_width,
                    const PaintRectBytes& rect) {
  const char* source = static_cast<const char*>(buffer);
  const int source_stride = buffer_width * 4;
  const int row_bytes = rect.width * 4;
  for (int row = 0; row < rect.height; ++row) {
    if (!WriteAllAt(fd,
                    source + (rect.y + row) * source_stride + rect.x * 4,
                    row_bytes,
                    static_cast<off_t>(rect.offset + static_cast<uint64_t>(row * row_bytes)))) {
      return false;
    }
  }
  return true;
}

int KeyCodeForName(const std::string& key) {
  if (key.size() == 1) {
    unsigned char c = key[0];
    if (c >= 'a' && c <= 'z') {
      return c - 'a' + 'A';
    }
    return c;
  }
  if (key.rfind("Key", 0) == 0 && key.size() == 4) {
    return key[3];
  }
  if (key == "Enter") return 13;
  if (key == "Backspace") return 8;
  if (key == "Tab") return 9;
  if (key == "Escape") return 27;
  if (key == " " || key == "Space") return 32;
  if (key == "ArrowLeft") return 37;
  if (key == "ArrowUp") return 38;
  if (key == "ArrowRight") return 39;
  if (key == "ArrowDown") return 40;
  if (key == "Delete") return 46;
  if (key == "Home") return 36;
  if (key == "End") return 35;
  if (key == "PageUp") return 33;
  if (key == "PageDown") return 34;
  if (key.size() >= 2 && key[0] == 'F') {
    const std::string number = key.substr(1);
    if (!number.empty() &&
        std::all_of(number.begin(), number.end(), [](unsigned char c) { return std::isdigit(c); })) {
      const int function_key = std::atoi(number.c_str());
      if (function_key >= 1 && function_key <= 24) {
        return 111 + function_key;
      }
    }
  }
  return 0;
}

std::u16string Utf8ToUtf16(const std::string& value) {
  std::u16string output;
  for (size_t i = 0; i < value.size();) {
    uint32_t cp = static_cast<unsigned char>(value[i++]);
    if ((cp & 0x80) == 0) {
    } else if ((cp & 0xe0) == 0xc0 && i < value.size()) {
      const uint32_t b1 = static_cast<unsigned char>(value[i++]);
      cp = ((cp & 0x1f) << 6) | (b1 & 0x3f);
    } else if ((cp & 0xf0) == 0xe0 && i + 1 < value.size()) {
      const uint32_t b1 = static_cast<unsigned char>(value[i++]);
      const uint32_t b2 = static_cast<unsigned char>(value[i++]);
      cp = ((cp & 0x0f) << 12) | ((b1 & 0x3f) << 6) | (b2 & 0x3f);
    } else if ((cp & 0xf8) == 0xf0 && i + 2 < value.size()) {
      const uint32_t b1 = static_cast<unsigned char>(value[i++]);
      const uint32_t b2 = static_cast<unsigned char>(value[i++]);
      const uint32_t b3 = static_cast<unsigned char>(value[i++]);
      cp = ((cp & 0x07) << 18) | ((b1 & 0x3f) << 12) |
           ((b2 & 0x3f) << 6) | (b3 & 0x3f);
    } else {
      continue;
    }
    if (cp <= 0xffff) {
      output.push_back(static_cast<char16_t>(cp));
    } else {
      cp -= 0x10000;
      output.push_back(static_cast<char16_t>(0xd800 + (cp >> 10)));
      output.push_back(static_cast<char16_t>(0xdc00 + (cp & 0x3ff)));
    }
  }
  return output;
}

cef_mouse_button_type_t MouseButtonFromString(const std::string& value) {
  if (value == "right") return MBT_RIGHT;
  if (value == "middle") return MBT_MIDDLE;
  return MBT_LEFT;
}

std::string CursorName(cef_cursor_type_t type) {
  switch (type) {
    case CT_HAND:
      return "pointer";
    case CT_IBEAM:
      return "text";
    case CT_CROSS:
      return "crosshair";
    case CT_MOVE:
      return "move";
    case CT_WAIT:
      return "wait";
    case CT_HELP:
      return "help";
    case CT_NOTALLOWED:
    case CT_NODROP:
      return "not-allowed";
    case CT_EASTWESTRESIZE:
    case CT_COLUMNRESIZE:
      return "ew-resize";
    case CT_NORTHSOUTHRESIZE:
    case CT_ROWRESIZE:
      return "ns-resize";
    case CT_NORTHEASTRESIZE:
      return "ne-resize";
    case CT_NORTHWESTRESIZE:
      return "nw-resize";
    case CT_SOUTHEASTRESIZE:
      return "se-resize";
    case CT_SOUTHWESTRESIZE:
      return "sw-resize";
    default:
      return "default";
  }
}
}  // namespace

FenestraOsrHandler::FenestraOsrHandler(std::string socket_path,
                               int width,
	                               int height,
	                               float scale,
	                               std::vector<std::string> bridge_commands,
	                               bool transparent_background,
	                               int active_frame_rate,
	                               int background_frame_rate)
	    : socket_path_(std::move(socket_path)),
	      width_(std::max(1, width)),
	      height_(std::max(1, height)),
	      scale_(std::max(0.25f, scale)),
	      bridge_commands_(bridge_commands.begin(), bridge_commands.end()),
	      transparent_background_(transparent_background),
	      active_frame_rate_(std::max(1, active_frame_rate)),
	      background_frame_rate_(std::max(1, background_frame_rate)) {
  if (!g_instance) {
    g_instance = this;
  }
  ConnectSocket();
  StartCommandReader();
  if (!bridge_commands_.empty()) {
    StartBridgeReader();
  }
}

FenestraOsrHandler::~FenestraOsrHandler() {
  if (socket_fd_ >= 0) {
    close(socket_fd_);
  }
  if (g_instance == this) {
    g_instance = nullptr;
  }
}

FenestraOsrHandler* FenestraOsrHandler::GetInstance() {
  return g_instance;
}

bool FenestraOsrHandler::ConnectSocket() {
  socket_fd_ = socket(AF_UNIX, SOCK_STREAM, 0);
  if (socket_fd_ < 0) {
    return false;
  }
  sockaddr_un addr{};
  addr.sun_family = AF_UNIX;
  if (socket_path_.size() >= sizeof(addr.sun_path)) {
    return false;
  }
  std::strncpy(addr.sun_path, socket_path_.c_str(), sizeof(addr.sun_path) - 1);
  if (connect(socket_fd_, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
    close(socket_fd_);
    socket_fd_ = -1;
    return false;
  }
  return true;
}

void FenestraOsrHandler::StartCommandReader() {
  if (socket_fd_ < 0) {
    return;
  }
  const int fd = socket_fd_;
  CefRefPtr<FenestraOsrHandler> self(this);
  std::thread([self, fd] {
    std::string pending;
    char buffer[2048];
    while (true) {
      const ssize_t n = recv(fd, buffer, sizeof(buffer), 0);
      if (n <= 0) {
        break;
      }
      pending.append(buffer, static_cast<size_t>(n));
      size_t newline = 0;
      while ((newline = pending.find('\n')) != std::string::npos) {
        std::string line = pending.substr(0, newline);
        pending.erase(0, newline + 1);
        CefPostTask(TID_UI, new OsrCommandTask(self, line));
      }
    }
  }).detach();
}

bool FenestraOsrHandler::SendMessage(uint32_t kind,
                                 uint32_t width,
                                 uint32_t height,
                                 int32_t x,
                                 int32_t y,
                                 const void* payload,
                                 uint32_t payload_len) {
  if (socket_fd_ < 0) {
    return false;
  }
  std::lock_guard<std::mutex> lock(socket_mutex_);
  std::vector<char> header(28, 0);
  header[0] = 'S';
  header[1] = 'K';
  header[2] = 'O';
  header[3] = 'R';
  PutU32(&header, 4, kind);
  PutU32(&header, 8, width);
  PutU32(&header, 12, height);
  PutI32(&header, 16, x);
  PutI32(&header, 20, y);
  PutU32(&header, 24, payload_len);
  return SendAll(socket_fd_, header.data(), header.size()) &&
         (payload_len == 0 ||
          SendAll(socket_fd_, static_cast<const char*>(payload), payload_len));
}

bool FenestraOsrHandler::SendMessageWithFd(uint32_t kind,
                                           uint32_t width,
                                           uint32_t height,
                                           int32_t x,
                                           int32_t y,
                                           const void* payload,
                                           uint32_t payload_len,
                                           int fd) {
  if (socket_fd_ < 0 || fd < 0) {
    return false;
  }
  std::lock_guard<std::mutex> lock(socket_mutex_);
  std::vector<char> header(28, 0);
  header[0] = 'S';
  header[1] = 'K';
  header[2] = 'O';
  header[3] = 'R';
  PutU32(&header, 4, kind);
  PutU32(&header, 8, width);
  PutU32(&header, 12, height);
  PutI32(&header, 16, x);
  PutI32(&header, 20, y);
  PutU32(&header, 24, payload_len);

  iovec iov{};
  iov.iov_base = header.data();
  iov.iov_len = header.size();
  alignas(cmsghdr) char control[CMSG_SPACE(sizeof(int))] = {};
  msghdr message{};
  message.msg_iov = &iov;
  message.msg_iovlen = 1;
  message.msg_control = control;
  message.msg_controllen = sizeof(control);
  cmsghdr* cmsg = CMSG_FIRSTHDR(&message);
  cmsg->cmsg_level = SOL_SOCKET;
  cmsg->cmsg_type = SCM_RIGHTS;
  cmsg->cmsg_len = CMSG_LEN(sizeof(int));
  std::memcpy(CMSG_DATA(cmsg), &fd, sizeof(int));

  const ssize_t sent = sendmsg(socket_fd_, &message, MSG_NOSIGNAL);
  return sent == static_cast<ssize_t>(header.size()) &&
         (payload_len == 0 ||
          SendAll(socket_fd_, static_cast<const char*>(payload), payload_len));
}

bool FenestraOsrHandler::SendPaintBatch(uint32_t kind,
                                        int32_t origin_x,
                                        int32_t origin_y,
                                        const void* buffer,
                                        int buffer_width,
                                        int buffer_height,
                                        const RectList& dirty_rects) {
  if (buffer_width <= 0 || buffer_height <= 0 || !buffer) {
    return false;
  }

  std::vector<CefRect> source_rects;
  if (dirty_rects.empty()) {
    source_rects.push_back(CefRect(0, 0, buffer_width, buffer_height));
  } else {
    source_rects.assign(dirty_rects.begin(), dirty_rects.end());
  }

  std::vector<PaintRectBytes> rects;
  uint64_t total_bytes = 0;
  for (const auto& rect : source_rects) {
    const int left = std::max(0, rect.x);
    const int top = std::max(0, rect.y);
    const int right = std::min(buffer_width, rect.x + rect.width);
    const int bottom = std::min(buffer_height, rect.y + rect.height);
    const int width = right - left;
    const int height = bottom - top;
    if (width <= 0 || height <= 0) {
      continue;
    }
    const uint64_t len = static_cast<uint64_t>(width) * height * 4;
    if (len > std::numeric_limits<uint32_t>::max()) {
      return false;
    }
    rects.push_back(PaintRectBytes{
        left,
        top,
        width,
        height,
        total_bytes,
        static_cast<uint32_t>(len),
    });
    total_bytes += len;
  }
  if (rects.empty()) {
    return true;
  }

  const size_t metadata_len = 4 + rects.size() * kBatchEntryLen;
  if (metadata_len > std::numeric_limits<uint32_t>::max()) {
    return false;
  }
  std::vector<char> metadata(metadata_len, 0);
  PutU32(&metadata, 0, static_cast<uint32_t>(rects.size()));
  for (size_t i = 0; i < rects.size(); ++i) {
    PutPaintEntry(&metadata, 4 + i * kBatchEntryLen, rects[i]);
  }

  const bool use_shared = total_bytes >= kSharedPaintThreshold;
  if (use_shared) {
    const int fd = CreateMemfd("fenestra-osr-paint");
    if (fd >= 0) {
      bool ok = ftruncate(fd, static_cast<off_t>(total_bytes)) == 0;
      for (const auto& rect : rects) {
        ok = ok && WritePaintRect(fd, buffer, buffer_width, rect);
      }
      ok = ok && lseek(fd, 0, SEEK_SET) >= 0;
      if (ok) {
        const uint32_t shared_kind =
            kind == kPopupFrame ? kPopupSharedBatch : kMainSharedBatch;
        ok = SendMessageWithFd(shared_kind,
                               static_cast<uint32_t>(buffer_width),
                               static_cast<uint32_t>(buffer_height),
                               origin_x,
                               origin_y,
                               metadata.data(),
                               static_cast<uint32_t>(metadata.size()),
                               fd);
      }
      close(fd);
      if (ok) {
        return true;
      }
    }
  }

  if (metadata_len + total_bytes > std::numeric_limits<uint32_t>::max()) {
    return false;
  }
  std::vector<char> payload(metadata_len + static_cast<size_t>(total_bytes), 0);
  std::memcpy(payload.data(), metadata.data(), metadata.size());
  char* data = payload.data() + metadata_len;
  for (const auto& rect : rects) {
    CopyPaintRect(data, buffer, buffer_width, rect);
  }
  const uint32_t batch_kind = kind == kPopupFrame ? kPopupBatch : kMainBatch;
  return SendMessage(batch_kind,
                     static_cast<uint32_t>(buffer_width),
                     static_cast<uint32_t>(buffer_height),
                     origin_x,
                     origin_y,
                     payload.data(),
                     static_cast<uint32_t>(payload.size()));
}

bool FenestraOsrHandler::OnCursorChange(CefRefPtr<CefBrowser> browser,
                                    CefCursorHandle cursor,
                                    cef_cursor_type_t type,
                                    const CefCursorInfo& custom_cursor_info) {
  const std::string name = CursorName(type);
  SendMessage(4, 0, 0, 0, 0, name.data(), static_cast<uint32_t>(name.size()));
  return true;
}

void FenestraOsrHandler::OnBeforeContextMenu(
    CefRefPtr<CefBrowser> browser,
    CefRefPtr<CefFrame> frame,
    CefRefPtr<CefContextMenuParams> params,
    CefRefPtr<CefMenuModel> model) {
  CEF_REQUIRE_UI_THREAD();
  model->Clear();
}

bool FenestraOsrHandler::OnContextMenuCommand(
    CefRefPtr<CefBrowser> browser,
    CefRefPtr<CefFrame> frame,
    CefRefPtr<CefContextMenuParams> params,
    int command_id,
    EventFlags event_flags) {
  CEF_REQUIRE_UI_THREAD();
  return true;
}

void FenestraOsrHandler::OnAfterCreated(CefRefPtr<CefBrowser> browser) {
	  CEF_REQUIRE_UI_THREAD();
	  browsers_.push_back(browser);
  if (native_popup_pending_ && browser_ && !browser_->IsSame(browser)) {
    native_popup_browser_ = browser;
    native_popup_pending_ = false;
  } else {
    browser_ = browser;
  }
	  browser->GetHost()->SetWindowlessFrameRate(active_frame_rate_);
	}

bool FenestraOsrHandler::DoClose(CefRefPtr<CefBrowser> browser) {
  CEF_REQUIRE_UI_THREAD();
  if (browsers_.size() == 1) {
    closing_ = true;
  }
  return false;
}

void FenestraOsrHandler::OnBeforeClose(CefRefPtr<CefBrowser> browser) {
  CEF_REQUIRE_UI_THREAD();
  if (IsNativePopupBrowser(browser)) {
    native_popup_browser_ = nullptr;
    native_popup_url_.clear();
    native_popup_pending_ = false;
    SendMessage(3, 0, 0, 0, 0, nullptr, 0);
  }
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

void FenestraOsrHandler::OnLoadError(CefRefPtr<CefBrowser> browser,
                                 CefRefPtr<CefFrame> frame,
                                 ErrorCode errorCode,
                                 const CefString& errorText,
                                 const CefString& failedUrl) {
  CEF_REQUIRE_UI_THREAD();
  if (errorCode == ERR_ABORTED) {
    return;
  }
  if (IsNativePopupBrowser(browser)) {
    std::cerr << "native popup load failed: " << errorText.ToString()
              << " url=" << failedUrl.ToString() << std::endl;
    CloseNativePopup();
    return;
  }
  std::stringstream body;
  body << "<!doctype html><meta charset=\"utf-8\"><body style=\"margin:0;"
          "font:14px system-ui;background:#111;color:#eee;padding:24px\">"
       << "<h2>Failed to load</h2><p>" << std::string(failedUrl) << "</p><p>"
       << std::string(errorText) << "</p></body>";
  frame->LoadURL(DataUri(body.str()));
}

void FenestraOsrHandler::OnLoadStart(CefRefPtr<CefBrowser> browser,
                                 CefRefPtr<CefFrame> frame,
                                 TransitionType transition_type) {
  CEF_REQUIRE_UI_THREAD();
  if (frame->IsMain()) {
    InstallTransparentBackground(frame);
    InstallBridge(browser, frame);
  }
}

void FenestraOsrHandler::OnLoadEnd(CefRefPtr<CefBrowser> browser,
                               CefRefPtr<CefFrame> frame,
                               int httpStatusCode) {
  CEF_REQUIRE_UI_THREAD();
  if (frame->IsMain()) {
    InstallTransparentBackground(frame);
    InstallBridge(browser, frame);
  }
}

bool FenestraOsrHandler::OnBeforeBrowse(CefRefPtr<CefBrowser> browser,
                                    CefRefPtr<CefFrame> frame,
                                    CefRefPtr<CefRequest> request,
                                    bool user_gesture,
                                    bool is_redirect) {
  CEF_REQUIRE_UI_THREAD();
  const std::string url = request->GetURL();
  return HandleWindowCommand(browser, url) || HandleBridgeCommand(browser, frame, url);
}

bool FenestraOsrHandler::GetScreenInfo(CefRefPtr<CefBrowser> browser,
                                   CefScreenInfo& screen_info) {
  screen_info.device_scale_factor = scale_;
  screen_info.depth = 32;
  screen_info.depth_per_component = 8;
  if (IsNativePopupBrowser(browser)) {
    screen_info.rect = CefRect(0, 0, native_popup_rect_.width, native_popup_rect_.height);
    screen_info.available_rect = screen_info.rect;
    return true;
  }
  screen_info.rect = CefRect(0, 0, width_, height_);
  screen_info.available_rect = CefRect(0, 0, width_, height_);
  return true;
}

void FenestraOsrHandler::GetViewRect(CefRefPtr<CefBrowser> browser, CefRect& rect) {
  if (IsNativePopupBrowser(browser)) {
    rect = CefRect(0, 0, native_popup_rect_.width, native_popup_rect_.height);
    return;
  }
  rect = CefRect(0, 0, width_, height_);
}

void FenestraOsrHandler::OnPopupShow(CefRefPtr<CefBrowser> browser, bool show) {
  if (IsNativePopupBrowser(browser)) {
    return;
  }
  if (!show) {
    SendMessage(3, 0, 0, 0, 0, nullptr, 0);
  }
}

void FenestraOsrHandler::OnPopupSize(CefRefPtr<CefBrowser> browser,
                                 const CefRect& rect) {
  if (IsNativePopupBrowser(browser)) {
    return;
  }
  if (popup_rect_.x != rect.x || popup_rect_.y != rect.y ||
      popup_rect_.width != rect.width || popup_rect_.height != rect.height) {
    SendMessage(3, 0, 0, 0, 0, nullptr, 0);
  }
  popup_rect_ = rect;
}

void FenestraOsrHandler::OnPaint(CefRefPtr<CefBrowser> browser,
	                             PaintElementType type,
	                             const RectList& dirtyRects,
	                             const void* buffer,
	                             int width,
	                             int height) {
  if (IsNativePopupBrowser(browser)) {
    if (!native_popup_visible_) {
      native_popup_visible_ = true;
      EmitBridgeEvent("\"popup.open\"", "{}");
    }
    SendPaintBatch(kPopupFrame,
                   native_popup_rect_.x,
                   native_popup_rect_.y,
                   buffer,
                   width,
                   height,
                   dirtyRects);
    return;
  }
	  if (suspended_) {
	    return;
	  }
	  const uint32_t kind = type == PET_POPUP ? kPopupFrame : kMainFrame;
  const int32_t x = type == PET_POPUP ? popup_rect_.x : 0;
  const int32_t y = type == PET_POPUP ? popup_rect_.y : 0;
  SendPaintBatch(kind, x, y, buffer, width, height, dirtyRects);
}

namespace {

std::string JsonEscape(const std::string& value) {
  std::string output;
  output.reserve(value.size() + 2);
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
        if (static_cast<unsigned char>(c) < 0x20) {
          char buffer[8];
          std::snprintf(buffer, sizeof(buffer), "\\u%04x", static_cast<unsigned char>(c));
          output += buffer;
        } else {
          output += c;
        }
        break;
    }
  }
  return output;
}

std::string JsonStringValue(const std::string& payload, const std::string& name) {
  const std::string needle = "\"" + name + "\"";
  size_t cursor = payload.find(needle);
  if (cursor == std::string::npos) {
    return "";
  }
  cursor = payload.find(':', cursor + needle.size());
  if (cursor == std::string::npos) {
    return "";
  }
  cursor = payload.find('"', cursor + 1);
  if (cursor == std::string::npos) {
    return "";
  }
  std::string output;
  for (++cursor; cursor < payload.size(); ++cursor) {
    const char c = payload[cursor];
    if (c == '"') {
      break;
    }
    if (c != '\\' || cursor + 1 >= payload.size()) {
      output += c;
      continue;
    }
    const char escaped = payload[++cursor];
    switch (escaped) {
      case '"':
      case '\\':
      case '/':
        output += escaped;
        break;
      case 'n':
        output += '\n';
        break;
      case 'r':
        output += '\r';
        break;
      case 't':
        output += '\t';
        break;
      default:
        output += escaped;
        break;
    }
  }
  return output;
}

int JsonIntValue(const std::string& payload,
                 const std::string& name,
                 int fallback) {
  const std::string needle = "\"" + name + "\"";
  size_t cursor = payload.find(needle);
  if (cursor == std::string::npos) {
    return fallback;
  }
  cursor = payload.find(':', cursor + needle.size());
  if (cursor == std::string::npos) {
    return fallback;
  }
  ++cursor;
  while (cursor < payload.size() &&
         std::isspace(static_cast<unsigned char>(payload[cursor]))) {
    ++cursor;
  }
  size_t end = cursor;
  if (end < payload.size() && payload[end] == '-') {
    ++end;
  }
  while (end < payload.size() &&
         std::isdigit(static_cast<unsigned char>(payload[end]))) {
    ++end;
  }
  if (end == cursor) {
    return fallback;
  }
  return std::atoi(payload.substr(cursor, end - cursor).c_str());
}

std::string FileUriToPath(const std::string& value) {
  const std::string prefix = "file://";
  if (value.rfind(prefix, 0) != 0) {
    return value;
  }
  std::string path = value.substr(prefix.size());
  const std::string host_prefix = "localhost/";
  if (path.rfind(host_prefix, 0) == 0) {
    path = path.substr(host_prefix.size());
  }
  std::string decoded;
  decoded.reserve(path.size());
  for (size_t i = 0; i < path.size(); ++i) {
    if (path[i] == '%' && i + 2 < path.size()) {
      char hex[3] = {path[i + 1], path[i + 2], 0};
      char* end = nullptr;
      const long byte = std::strtol(hex, &end, 16);
      if (end == hex + 2) {
        decoded.push_back(static_cast<char>(byte));
        i += 2;
        continue;
      }
    }
    decoded.push_back(path[i] == '?' || path[i] == '#' ? '\0' : path[i]);
  }
  return decoded;
}

std::string BuildFileDragPayload(const std::vector<std::string>& paths) {
  std::string output = "{\"paths\":[";
  bool first = true;
  for (const auto& path : paths) {
    if (!first) {
      output += ",";
    }
    first = false;
    output += '"';
    output += JsonEscape(path);
    output += '"';
  }
  output += "]}";
  return output;
}

}  // namespace

bool FenestraOsrHandler::StartDragging(CefRefPtr<CefBrowser> browser,
                                   CefRefPtr<CefDragData> drag_data,
                                   DragOperationsMask allowed_ops,
                                   int x,
                                   int y) {
  CEF_REQUIRE_UI_THREAD();
  if (!browser || !drag_data || socket_fd_ < 0) {
    return false;
  }

  std::vector<std::string> paths;

  if (drag_data->IsFile()) {
    std::vector<CefString> file_paths;
    if (drag_data->GetFilePaths(file_paths) && !file_paths.empty()) {
      for (const auto& file_path : file_paths) {
        paths.push_back(FileUriToPath(file_path.ToString()));
      }
    }
    if (paths.empty()) {
      const std::string file_name = drag_data->GetFileName().ToString();
      if (!file_name.empty()) {
        paths.push_back(FileUriToPath(file_name));
      }
    }
  }

  if (paths.empty()) {
    const std::string fragment_text = drag_data->GetFragmentText().ToString();
    const std::string link_url = drag_data->GetLinkURL().ToString();
    std::stringstream stream;
    if (!fragment_text.empty()) {
      stream << fragment_text;
    } else if (!link_url.empty()) {
      stream << link_url;
    }
    std::string line;
    while (std::getline(stream, line)) {
      std::string trimmed = line;
      while (!trimmed.empty() && (trimmed.back() == '\r' || trimmed.back() == '\n')) {
        trimmed.pop_back();
      }
      if (trimmed.empty()) continue;
      paths.push_back(FileUriToPath(trimmed));
    }
  }

  if (paths.empty()) {
    return false;
  }

  const std::string payload = BuildFileDragPayload(paths);
  SendMessage(kFileDragRequested, 0, 0, x, y, payload.data(),
              static_cast<uint32_t>(payload.size()));

  // Until a native X11/Wayland DnD backend is wired up in the host, the
  // request is reported but no system drag is started. Return false so CEF
  // doesn't keep waiting for DragSource*Ended callbacks.
  return false;
}

void FenestraOsrHandler::UpdateDragCursor(CefRefPtr<CefBrowser> browser,
                                      DragOperation operation) {
  // No-op: cursor changes are driven by the host's window manager.
}

void FenestraOsrHandler::HandleControlLine(const std::string& line) {
  CEF_REQUIRE_UI_THREAD();
  const auto parts = Split(line, '\t');
  if (parts.empty() || !browser_) {
    return;
  }
  CefRefPtr<CefBrowser> target_browser = browser_;
  int pointer_x = parts.size() >= 3 ? std::atoi(parts[1].c_str()) : 0;
  int pointer_y = parts.size() >= 3 ? std::atoi(parts[2].c_str()) : 0;
  const bool pointer_in_popup =
      native_popup_browser_ &&
      pointer_x >= native_popup_rect_.x &&
      pointer_y >= native_popup_rect_.y &&
      pointer_x < native_popup_rect_.x + native_popup_rect_.width &&
      pointer_y < native_popup_rect_.y + native_popup_rect_.height;
  if (pointer_in_popup &&
      (parts[0] == "mouse_move" || parts[0] == "mouse_click" ||
       parts[0] == "mouse_wheel" || parts[0] == "mouse_navigation")) {
    target_browser = native_popup_browser_;
    pointer_x -= native_popup_rect_.x;
    pointer_y -= native_popup_rect_.y;
  } else if (native_popup_browser_ && parts[0] == "mouse_click") {
    CloseNativePopup();
  }
  CefRefPtr<CefBrowserHost> host = target_browser->GetHost();
  if (parts[0] == "resize" && parts.size() >= 4) {
    width_ = std::max(1, std::atoi(parts[1].c_str()));
    height_ = std::max(1, std::atoi(parts[2].c_str()));
    scale_ = std::max(0.25f, static_cast<float>(std::atof(parts[3].c_str())));
    host->NotifyScreenInfoChanged();
    host->WasResized();
  } else if (parts[0] == "mouse_move" && parts.size() >= 5) {
    CefMouseEvent event;
    event.x = pointer_x;
    event.y = pointer_y;
    event.modifiers = std::strtoul(parts[3].c_str(), nullptr, 10);
    host->SendMouseMoveEvent(event, std::atoi(parts[4].c_str()) != 0);
  } else if (parts[0] == "mouse_click" && parts.size() >= 7) {
    CefMouseEvent event;
    event.x = pointer_x;
    event.y = pointer_y;
    const auto button = MouseButtonFromString(parts[3]);
    event.modifiers = std::strtoul(parts[4].c_str(), nullptr, 10);
    const bool up = std::atoi(parts[5].c_str()) != 0;
    const int click_count = std::max(1, std::atoi(parts[6].c_str()));
    host->SendMouseClickEvent(event, button, up, click_count);
    if (up && parts[3] == "right") {
      const std::string script =
          std::string("(function(){const target=document.elementFromPoint(") +
          std::to_string(event.x) + "," + std::to_string(event.y) +
          ")||document.body||window;const init={bubbles:true,cancelable:true,button:2,buttons:0,clientX:" +
          std::to_string(event.x) + ",clientY:" + std::to_string(event.y) +
          ",screenX:" + std::to_string(event.x) + ",screenY:" +
          std::to_string(event.y) + ",ctrlKey:" +
          ((event.modifiers & (1 << 2)) ? "true" : "false") + ",shiftKey:" +
          ((event.modifiers & (1 << 1)) ? "true" : "false") + ",altKey:" +
          ((event.modifiers & (1 << 3)) ? "true" : "false") + ",metaKey:" +
          ((event.modifiers & (1 << 7)) ? "true" : "false") +
          "};target.dispatchEvent(new MouseEvent('contextmenu',init));})();";
      target_browser->GetMainFrame()->ExecuteJavaScript(
          script, target_browser->GetMainFrame()->GetURL(), 0);
    }
  } else if (parts[0] == "mouse_navigation" && parts.size() >= 5) {
    const int x = pointer_x;
    const int y = pointer_y;
    const int button = std::atoi(parts[3].c_str());
    const uint32_t modifiers = std::strtoul(parts[4].c_str(), nullptr, 10);
    const std::string script =
        std::string("(function(){const target=document.elementFromPoint(") + std::to_string(x) +
        "," + std::to_string(y) + ")||window;const init={bubbles:true,cancelable:true,button:" +
        std::to_string(button) + ",buttons:0,clientX:" + std::to_string(x) +
        ",clientY:" + std::to_string(y) + ",screenX:" + std::to_string(x) +
        ",screenY:" + std::to_string(y) + ",ctrlKey:" +
        ((modifiers & (1 << 2)) ? "true" : "false") + ",shiftKey:" +
        ((modifiers & (1 << 1)) ? "true" : "false") + ",altKey:" +
        ((modifiers & (1 << 3)) ? "true" : "false") + ",metaKey:" +
        ((modifiers & (1 << 7)) ? "true" : "false") +
        "};const up=new MouseEvent('mouseup',init);const aux=new MouseEvent('auxclick',init);"
        "const canceled=(target.dispatchEvent(up)===false)||up.defaultPrevented||"
        "(target.dispatchEvent(aux)===false)||aux.defaultPrevented;"
        "if(!canceled){if(" +
        std::to_string(button) +
        "===3)history.back();else if(" + std::to_string(button) +
        "===4)history.forward();}})();";
    target_browser->GetMainFrame()->ExecuteJavaScript(
        script, target_browser->GetMainFrame()->GetURL(), 0);
  } else if (parts[0] == "mouse_wheel" && parts.size() >= 6) {
    CefMouseEvent event;
    event.x = pointer_x;
    event.y = pointer_y;
    const int dx = std::atoi(parts[3].c_str());
    const int dy = std::atoi(parts[4].c_str());
    event.modifiers = std::strtoul(parts[5].c_str(), nullptr, 10);
    host->SendMouseWheelEvent(event, dx, dy);
  } else if (parts[0] == "key" && parts.size() >= 6) {
    const bool pressed = std::atoi(parts[1].c_str()) != 0;
    const std::string key = DecodeUriComponent(parts[2]);
    const std::string text = DecodeUriComponent(parts[3]);
    const uint32_t modifiers = std::strtoul(parts[4].c_str(), nullptr, 10);
    const int key_code = KeyCodeForName(key);
    CefKeyEvent event;
    event.type = pressed ? KEYEVENT_RAWKEYDOWN : KEYEVENT_KEYUP;
    event.modifiers = modifiers;
    event.windows_key_code = key_code;
    host->SendKeyEvent(event);
    if (pressed && !text.empty()) {
      for (char16_t ch : Utf8ToUtf16(text)) {
        CefKeyEvent char_event;
        char_event.type = KEYEVENT_CHAR;
        char_event.modifiers = modifiers;
        char_event.windows_key_code = ch;
        char_event.native_key_code = ch;
        char_event.character = ch;
        char_event.unmodified_character = ch;
        host->SendKeyEvent(char_event);
      }
    }
	  } else if (parts[0] == "focus" && parts.size() >= 2) {
	    host->SetFocus(std::atoi(parts[1].c_str()) != 0);
	  } else if (parts[0] == "lifecycle" && parts.size() >= 3) {
	    const std::string reason =
	        parts.size() >= 4 ? DecodeUriComponent(parts[3]) : "";
	    ApplyLifecycle(parts[1], std::max(1, std::atoi(parts[2].c_str())),
	                   reason);
	  } else if (parts[0] == "close") {
    host->CloseBrowser(false);
    CefPostDelayedTask(TID_UI, new QuitTask, 250);
	  }
	}

void FenestraOsrHandler::ApplyLifecycle(const std::string& state,
                                        int frame_rate,
                                        const std::string& reason) {
  CEF_REQUIRE_UI_THREAD();
  if (!browser_) {
    return;
  }
  CefRefPtr<CefBrowserHost> host = browser_->GetHost();
  if (state == "active") {
    suspended_ = false;
    host->WasHidden(false);
    host->SetWindowlessFrameRate(std::max(1, frame_rate));
    host->WasResized();
    DispatchLifecycle("active", reason);
    return;
  }
  if (state == "hibernate") {
    DispatchLifecycle("hibernate", reason);
    suspended_ = true;
    host->SetWindowlessFrameRate(std::max(1, background_frame_rate_));
    host->WasHidden(true);
    return;
  }
  suspended_ = true;
  host->SetWindowlessFrameRate(std::max(1, frame_rate));
  host->WasHidden(true);
  DispatchLifecycle("suspended", reason);
}

void FenestraOsrHandler::DispatchLifecycle(const std::string& state,
                                           const std::string& reason) {
  CEF_REQUIRE_UI_THREAD();
  const std::string script =
      "window.__fenestraLifecycleSet&&window.__fenestraLifecycleSet(" +
      JsString(state) + "," + JsString(reason) + ");";
  for (auto& browser : browsers_) {
    browser->GetMainFrame()->ExecuteJavaScript(
        script, browser->GetMainFrame()->GetURL(), 0);
  }
}

bool FenestraOsrHandler::HandleWindowCommand(CefRefPtr<CefBrowser> browser,
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
  if (command == "close") {
    RequestNativeClose();
    browser->GetHost()->CloseBrowser(false);
    CefPostDelayedTask(TID_UI, new QuitTask, 250);
  } else if (command == "start-drag" || command == "drag") {
    SendMessage(6, 0, 0, 0, 0, nullptr, 0);
  } else if (command == "minimize") {
    SendMessage(7, 0, 0, 0, 0, nullptr, 0);
	  } else if (command == "maximize" || command == "toggle-maximize") {
	    SendMessage(8, 0, 0, 0, 0, nullptr, 0);
	  } else if (command == "show") {
	    SendMessage(9, 0, 0, 0, 0, nullptr, 0);
	  } else if (command == "hide") {
	    SendMessage(10, 0, 0, 0, 0, nullptr, 0);
	  } else if (command == "focus") {
	    SendMessage(11, 0, 0, 0, 0, nullptr, 0);
	  }
  return true;
}

bool FenestraOsrHandler::IsNativePopupBrowser(CefRefPtr<CefBrowser> browser) const {
  if (!browser) {
    return false;
  }
  if (native_popup_browser_ && native_popup_browser_->IsSame(browser)) {
    return true;
  }
  if (!native_popup_pending_ || native_popup_url_.empty()) {
    return false;
  }
  if (browser_ && browser_->IsSame(browser)) {
    return false;
  }
  CefRefPtr<CefFrame> frame = browser->GetMainFrame();
  return frame && std::string(frame->GetURL()) == native_popup_url_;
}

bool FenestraOsrHandler::OpenNativePopup(const std::string& html,
                                         int x,
                                         int y,
                                         int width,
                                         int height) {
  CEF_REQUIRE_UI_THREAD();
  if (html.empty()) {
    CloseNativePopup();
    return false;
  }
  CloseNativePopup();
  native_popup_rect_ = CefRect(x, y, std::max(1, width), std::max(1, height));
  native_popup_pending_ = true;
  native_popup_visible_ = false;
  native_popup_url_ = DataUri(html);

  CefBrowserSettings browser_settings;
  browser_settings.windowless_frame_rate = std::max(1, active_frame_rate_);
  browser_settings.background_color = CefColorSetARGB(0, 0, 0, 0);

  CefWindowInfo window_info;
  window_info.SetAsWindowless(kNullWindowHandle);
  const bool created = CefBrowserHost::CreateBrowser(
      window_info, this, native_popup_url_, browser_settings, nullptr, nullptr);
  if (!created) {
    native_popup_pending_ = false;
    native_popup_visible_ = false;
    native_popup_url_.clear();
  }
  return created;
}

void FenestraOsrHandler::CloseNativePopup() {
  CEF_REQUIRE_UI_THREAD();
  const bool had_popup = native_popup_pending_ || native_popup_browser_;
  native_popup_pending_ = false;
  native_popup_visible_ = false;
  native_popup_url_.clear();
  if (native_popup_browser_) {
    CefRefPtr<CefBrowser> popup = native_popup_browser_;
    native_popup_browser_ = nullptr;
    popup->GetHost()->CloseBrowser(true);
  }
  if (had_popup) {
    EmitBridgeEvent("\"popup.close\"", "{}");
  }
  SendMessage(3, 0, 0, 0, 0, nullptr, 0);
}

bool FenestraOsrHandler::HandleBridgeCommand(CefRefPtr<CefBrowser> browser,
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
  if (command == "fenestra.popup.open") {
    const bool opened = OpenNativePopup(JsonStringValue(payload, "html"),
                                        JsonIntValue(payload, "x", 0),
                                        JsonIntValue(payload, "y", 0),
                                        JsonIntValue(payload, "width", 1),
                                        JsonIntValue(payload, "height", 1));
    if (!opened) {
      ResolveBridgeResponse(browser_id, request_id, false,
                            "{\"message\":\"Failed to create native popup\"}");
      return true;
    }
    ResolveBridgeResponse(browser_id, request_id, true, "{\"accepted\":true}");
    return true;
  }
  if (command == "fenestra.popup.close") {
    CloseNativePopup();
    ResolveBridgeResponse(browser_id, request_id, true, "{}");
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

void FenestraOsrHandler::RequestNativeClose() {
  SendMessage(5, 0, 0, 0, 0, nullptr, 0);
}

void FenestraOsrHandler::InstallBridge(CefRefPtr<CefBrowser> browser,
	                                   CefRefPtr<CefFrame> frame) {
	  frame->ExecuteJavaScript(BridgeInstallScript(bridge_commands_), frame->GetURL(),
	                           0);
	}

void FenestraOsrHandler::InstallTransparentBackground(CefRefPtr<CefFrame> frame) {
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

void FenestraOsrHandler::ResolveBridgeResponse(const std::string& browser_id,
                                           const std::string& request_id,
                                           bool ok,
                                           const std::string& payload) {
  CEF_REQUIRE_UI_THREAD();
  const int expected_id = std::atoi(browser_id.c_str());
  CefRefPtr<CefBrowser> target;
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

void FenestraOsrHandler::EmitBridgeEvent(const std::string& name_json,
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

void CreateFenestraOsrBrowser(CefRefPtr<CefCommandLine> command_line) {
  const std::string url_value = command_line->GetSwitchValue("url");
  const std::string url =
      url_value.empty() ? "about:blank" : std::string(url_value);
	  const int width = std::max(1, SwitchInt(command_line, "fenestra-width", 800));
	  const int height = std::max(1, SwitchInt(command_line, "fenestra-height", 600));
	  const float scale = SwitchFloat(command_line, "fenestra-scale", 1.0f);
	  const int active_frame_rate =
	      std::max(1, SwitchInt(command_line, "fenestra-active-frame-rate", 60));
	  const int background_frame_rate =
	      std::max(1, SwitchInt(command_line, "fenestra-background-frame-rate", 5));
	  const std::string socket_path = command_line->GetSwitchValue("fenestra-osr-socket");

	  CefBrowserSettings browser_settings;
	  browser_settings.windowless_frame_rate = active_frame_rate;
  if (command_line->HasSwitch("fenestra-transparent")) {
    browser_settings.background_color = CefColorSetARGB(0, 0, 0, 0);
  }

	  CefWindowInfo window_info;
	  window_info.SetAsWindowless(kNullWindowHandle);
	  CefRefPtr<FenestraOsrHandler> handler(new FenestraOsrHandler(
	      socket_path, width, height, scale, BridgeCommands(command_line),
	      command_line->HasSwitch("fenestra-transparent"), active_frame_rate,
	      background_frame_rate));
  CefBrowserHost::CreateBrowser(window_info, handler, url, browser_settings,
                                nullptr, nullptr);
}
