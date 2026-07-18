#ifndef __CCC_MATH_WRAPPER_H
#define __CCC_MATH_WRAPPER_H

#if defined(__CCC__) && defined(__APPLE__) && defined(__aarch64__)
/*
 * Current Apple SDK math.h defines private classification helpers in terms of
 * a wider builtin family than CCC advertises.  Supply only the expression
 * forms needed while that header is parsed. The Darwin arm64 profile makes
 * long double binary64, so the private long-double spelling is representation
 * exact. Public functions such as fabsl remain SDK declarations; these
 * temporary spellings never escape this include or enter __has_builtin.
 */
#define __builtin_fabsf(value) ((value) < 0.0f ? -(value) : (value))
#define __builtin_fabs(value) ((value) < 0.0 ? -(value) : (value))
#define __builtin_fabsl(value) ((value) < 0.0L ? -(value) : (value))
#define __builtin_inf() __builtin_huge_val()
#define __builtin_infl() __builtin_huge_val()
#define __CCC_APPLE_MATH_PRIVATE_BUILTINS 1
#endif

#include_next <math.h>
#include <float.h>

#ifdef __CCC_APPLE_MATH_PRIVATE_BUILTINS
#undef __CCC_APPLE_MATH_PRIVATE_BUILTINS
#undef __builtin_infl
#undef __builtin_inf
#undef __builtin_fabsl
#undef __builtin_fabs
#undef __builtin_fabsf
#endif

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
