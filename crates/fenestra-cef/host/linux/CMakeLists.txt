cmake_minimum_required(VERSION 3.21)

project(fenestra_cef_host)

set(CMAKE_CONFIGURATION_TYPES Debug Release)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(NOT CEF_ROOT)
  message(FATAL_ERROR "CEF_ROOT is required")
endif()

set(CMAKE_MODULE_PATH ${CMAKE_MODULE_PATH} "${CEF_ROOT}/cmake")
find_package(CEF REQUIRED)

add_subdirectory(${CEF_LIBCEF_DLL_WRAPPER_PATH} libcef_dll_wrapper)
ADD_LOGICAL_TARGET("libcef_lib" "${CEF_LIB_DEBUG}" "${CEF_LIB_RELEASE}")

add_executable(fenestra-cef-host
  main.cc
  app.cc
  app.h
  handler.cc
  handler.h
  osr_handler.cc
  osr_handler.h
)

SET_EXECUTABLE_TARGET_PROPERTIES(fenestra-cef-host)
target_include_directories(fenestra-cef-host PRIVATE "${CEF_ROOT}")
add_dependencies(fenestra-cef-host libcef_dll_wrapper)
target_link_libraries(fenestra-cef-host libcef_lib libcef_dll_wrapper ${CEF_STANDARD_LIBS})

set_target_properties(fenestra-cef-host PROPERTIES INSTALL_RPATH "$ORIGIN")
set_target_properties(fenestra-cef-host PROPERTIES BUILD_WITH_INSTALL_RPATH TRUE)
set_target_properties(fenestra-cef-host PROPERTIES RUNTIME_OUTPUT_DIRECTORY "${CEF_ROOT}/Release")

COPY_FILES("fenestra-cef-host" "${CEF_BINARY_FILES}" "${CEF_BINARY_DIR}" "${CEF_ROOT}/Release")
COPY_FILES("fenestra-cef-host" "${CEF_RESOURCE_FILES}" "${CEF_RESOURCE_DIR}" "${CEF_ROOT}/Release")
if(EXISTS "${CEF_BINARY_DIR}/libminigbm.so")
  COPY_FILES("fenestra-cef-host" "libminigbm.so" "${CEF_BINARY_DIR}" "${CEF_ROOT}/Release")
endif()

SET_LINUX_SUID_PERMISSIONS("fenestra-cef-host" "${CEF_ROOT}/Release/chrome-sandbox")
