#ifndef FINDERMAXXING_INPUT_H
#define FINDERMAXXING_INPUT_H

// Keyboard state at the moment of a drop: QML's DragEvent doesn't carry the
// modifiers, and they decide between copying and moving.

#include <cstdint>

std::uint32_t keyboardModifiers();

#endif // FINDERMAXXING_INPUT_H
