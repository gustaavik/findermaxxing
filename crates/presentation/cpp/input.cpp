#include "presentation/cpp/input.h"

#include <QtGui/QGuiApplication>

std::uint32_t keyboardModifiers()
{
    return static_cast<std::uint32_t>(QGuiApplication::queryKeyboardModifiers().toInt());
}
