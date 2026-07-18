#if defined(__APPLE__)
#include <libunwind.h>
#else
#include <unwind.h>
#endif
#include <stdarg.h>

#include "abi_types.h"

#if defined(__GNUC__) || defined(__clang__)
#define ORACLE_NOINLINE __attribute__((noinline))
#else
#define ORACLE_NOINLINE
#endif

#if defined(__APPLE__)
static ORACLE_NOINLINE int count_frames(void) {
    unw_context_t context;
    unw_cursor_t cursor;
    int count = 0;
    if (unw_getcontext(&context) < 0) return -1;
    if (unw_init_local(&cursor, &context) < 0) return -1;
    while (count < 64 && unw_step(&cursor) > 0) count += 1;
    return count;
}
#else
static _Unwind_Reason_Code count_frame(
    struct _Unwind_Context *context, void *argument) {
    int *count = argument;
    (void)context;
    *count += 1;
    return _URC_NO_REASON;
}

static ORACLE_NOINLINE int count_frames(void) {
    int count = 0;
    _Unwind_Backtrace(count_frame, &count);
    return count;
}
#endif

ORACLE_NOINLINE int target_oracle_unwind_probe(int marker) {
    int count = count_frames();
    if (marker < 1 || marker > 3) return 63;
    return count >= 4 ? 0 : 64;
}

int ref_unwind_variadic(int marker, ...) {
    va_list arguments;
    int payload;
    va_start(arguments, marker);
    payload = va_arg(arguments, int);
    va_end(arguments);
    if (payload != 9) return 65;
    return target_oracle_unwind_probe(marker);
}
