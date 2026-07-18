#include <stdarg.h>
#include <stdio.h>

#include "abi_types.h"

int ccc_format(char *buffer, unsigned long size, const char *format, ...) {
    va_list arguments;
    int result;
    va_start(arguments, format);
    result = vsnprintf(buffer, size, format, arguments);
    va_end(arguments);
    return result;
}
