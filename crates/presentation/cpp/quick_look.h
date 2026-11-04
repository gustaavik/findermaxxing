#ifndef FINDERMAXXING_QUICK_LOOK_H
#define FINDERMAXXING_QUICK_LOOK_H

// macOS Quick Look panel (QLPreviewPanel) for the focused window.
// On other platforms these report "unavailable" and do nothing.

#include "rust/cxx.h"

// Opens the panel on `fileUrls`, or closes it if it is already open.
// Returns false when Quick Look is unavailable on this platform.
bool toggleQuickLook(const rust::Vec<rust::String>& fileUrls);

// Shows `fileUrls` in the panel if it is open (selection followed it).
void refreshQuickLook(const rust::Vec<rust::String>& fileUrls);

#endif // FINDERMAXXING_QUICK_LOOK_H
