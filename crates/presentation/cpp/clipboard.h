#ifndef FINDERMAXXING_CLIPBOARD_H
#define FINDERMAXXING_CLIPBOARD_H

// The system clipboard holds copied items as file URLs, which is what Finder,
// Nautilus and Dolphin exchange. Rust builds and parses the URLs.

#include "rust/cxx.h"

void setClipboardUrls(const rust::Vec<rust::String>& urls);
rust::Vec<rust::String> clipboardUrls();

#endif // FINDERMAXXING_CLIPBOARD_H
