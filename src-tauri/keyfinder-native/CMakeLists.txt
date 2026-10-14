cmake_minimum_required(VERSION 3.16)
project(burrata_keyfinder_native LANGUAGES C CXX)
include(GNUInstallDirs)

set(BUILD_SHARED_LIBS OFF CACHE BOOL "Build bundled dependencies statically" FORCE)
set(BUILD_TESTING OFF CACHE BOOL "Disable upstream tests in application build" FORCE)
set(BUILD_TESTS OFF CACHE BOOL "Disable FFTW tests in application build" FORCE)
set(ENABLE_FLOAT OFF CACHE BOOL "Use the double precision FFTW API" FORCE)
set(ENABLE_LONG_DOUBLE OFF CACHE BOOL "Disable unused long double FFTW API" FORCE)
set(ENABLE_QUAD_PRECISION OFF CACHE BOOL "Disable unused quad precision FFTW API" FORCE)
set(DISABLE_FORTRAN ON CACHE BOOL "Disable unused Fortran bindings" FORCE)
set(ENABLE_THREADS OFF CACHE BOOL "Keep analyzer deterministic and single threaded" FORCE)
set(ENABLE_OPENMP OFF CACHE BOOL "Keep analyzer single threaded" FORCE)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
set(CMAKE_INSTALL_LIBDIR lib CACHE STRING "Use a stable native library path" FORCE)

add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/../vendor/fftw fftw)
add_library(FFTW3::fftw3 ALIAS fftw3)
set(FFTW3_LIBRARY fftw3)
set(FFTW3_INCLUDE_DIR ${CMAKE_CURRENT_SOURCE_DIR}/../vendor/fftw/api)
set(FFTW3_FOUND TRUE)
target_include_directories(fftw3 PUBLIC
  $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/../vendor/fftw/api>
  $<BUILD_INTERFACE:${CMAKE_CURRENT_BINARY_DIR}/fftw>
  $<INSTALL_INTERFACE:include>)
add_subdirectory(${CMAKE_CURRENT_SOURCE_DIR}/../vendor/libkeyfinder keyfinder)

add_library(burrata_keyfinder_ffi STATIC keyfinder_ffi.cpp)
target_include_directories(burrata_keyfinder_ffi PRIVATE
  ${CMAKE_CURRENT_SOURCE_DIR}/../vendor/libkeyfinder/src)
target_link_libraries(burrata_keyfinder_ffi PUBLIC keyfinder)
target_compile_features(burrata_keyfinder_ffi PRIVATE cxx_std_11)
install(TARGETS burrata_keyfinder_ffi
  ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR}
  LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR}
  RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
