#include "presentation/cpp/clock.h"

#include <QtCore/QDateTime>

std::int32_t utcOffsetSeconds()
{
    return QDateTime::currentDateTime().offsetFromUtc();
}
