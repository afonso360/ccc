#ifndef __CCC_MATH_WRAPPER_H
#define __CCC_MATH_WRAPPER_H

#include_next <math.h>
#include <float.h>

#if defined(__CCC__) && !defined(__cplusplus)
#undef isfinite
#undef isinf
#undef isnan

#define __ccc_math_isfinite(value)                                           \
    __extension__({                                                          \
        double __ccc_math_value = (value);                                   \
        __ccc_math_value == __ccc_math_value &&                              \
            __ccc_math_value >= -DBL_MAX && __ccc_math_value <= DBL_MAX;     \
    })
#define __ccc_math_isinf(value)                                              \
    __extension__({                                                          \
        double __ccc_math_value = (value);                                   \
        __ccc_math_value == __ccc_math_value &&                              \
            !(__ccc_math_value >= -DBL_MAX && __ccc_math_value <= DBL_MAX);  \
    })
#define __ccc_math_isnan(value)                                              \
    __extension__({                                                          \
        double __ccc_math_value = (value);                                   \
        __ccc_math_value != __ccc_math_value;                                \
    })

#define isfinite(value) __ccc_math_isfinite(value)
#define isinf(value) __ccc_math_isinf(value)
#define isnan(value) __ccc_math_isnan(value)
#endif

#endif
