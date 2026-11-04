#ifndef FINDERMAXXING_CLOCK_H
#define FINDERMAXXING_CLOCK_H

// The user's current UTC offset, for resolving "yesterday" or "last spring"
// in their time zone (Rust's std has no time zone database).

#include <cstdint>

std::int32_t utcOffsetSeconds();

#endif // FINDERMAXXING_CLOCK_H
