#ifndef FINDERMAXXING_BOOTSTRAP_H
#define FINDERMAXXING_BOOTSTRAP_H
// Include guard instead of #pragma once: qmltyperegistrar reaches this header
// through two different paths (crate source and exported copy).

#include <cstdint>

#include "rust/cxx.h"

struct AppServices;

// Creates the Qt application and QML engine, hands `services` to the root
// window, and runs the event loop. Returns the process exit code.
std::int32_t runApplication(rust::Box<AppServices> services);

#endif // FINDERMAXXING_BOOTSTRAP_H
