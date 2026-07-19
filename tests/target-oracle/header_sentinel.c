#include <float.h>
#include <limits.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>

#if defined(__APPLE__)
#include <Availability.h>
#include <TargetConditionals.h>
#else
#include <unistd.h>
#endif

_Static_assert(sizeof(size_t) == 8, "size_t must be 64-bit");
_Static_assert(sizeof(ptrdiff_t) == 8, "ptrdiff_t must be 64-bit");
_Static_assert(sizeof(void *) == 8, "pointers must be 64-bit");
_Static_assert(sizeof(uint64_t) == 8, "uint64_t must be 64-bit");
_Static_assert(CHAR_BIT == 8, "bytes must contain eight bits");
_Static_assert(DBL_MANT_DIG == 53, "double must use binary64");

#if defined(__APPLE__)
_Static_assert(TARGET_OS_OSX == 1, "the Darwin profile must select macOS");
_Static_assert(TARGET_CPU_ARM64 == 1, "the Darwin profile must select arm64");
#if __APPLE_CC__ != 6000
#error unexpected Apple compiler compatibility identity
#endif
#if __ENVIRONMENT_MAC_OS_X_VERSION_MIN_REQUIRED__ != 110000
#error unexpected minimum macOS version
#endif
#if __ENVIRONMENT_OS_VERSION_MIN_REQUIRED__ != 110000
#error unexpected minimum Apple OS version
#endif
#else
_Static_assert(sizeof(long double) == 16, "Linux long double must use a 16-byte object");
#endif

int main(void) {
    char buffer[32];
    uint64_t value = UINT64_C(42);
    int length = snprintf(buffer, sizeof(buffer), "%llu", (unsigned long long)value);
    if (length != 2 || strlen(buffer) != 2 || strcmp(buffer, "42") != 0) return 71;
    if (puts(buffer) < 0) return 72;
#if !defined(__APPLE__)
    if (getpid() <= (pid_t)0) return 73;
#endif
    return EXIT_SUCCESS;
}
