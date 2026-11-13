#ifndef FINDERMAXXING_UPDATER_H
#define FINDERMAXXING_UPDATER_H

// Sparkle 2 on macOS (ADR 0031). Sparkle.framework is loaded from the app
// bundle at run time, so development builds (no bundle, no framework) just
// report "unavailable". Elsewhere these are no-ops; Linux updates are
// handled in Rust (AppImage) or by Flatpak.

#include "rust/cxx.h"

// Starts Sparkle once (it schedules checks only if the user agreed).
// Returns false when Sparkle isn't available in this build.
bool startSparkle();
// "Check for Updates…": Sparkle's own window with its progress and answer.
void checkForUpdatesNow();
bool automaticUpdateChecks();
void setAutomaticUpdateChecks(bool enabled);

#endif // FINDERMAXXING_UPDATER_H
