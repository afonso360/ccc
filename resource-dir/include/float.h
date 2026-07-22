#ifndef __CCC_FLOAT_WRAPPER_H
#define __CCC_FLOAT_WRAPPER_H

#include_next <float.h>

#if defined(__CCC__) && defined(__STDC_WANT_IEC_60559_TYPES_EXT__) && \
    defined(__FLT16_MANT_DIG__)
#ifndef FLT16_MANT_DIG
#define FLT16_MANT_DIG __FLT16_MANT_DIG__
#define FLT16_DECIMAL_DIG __FLT16_DECIMAL_DIG__
#define FLT16_DIG __FLT16_DIG__
#define FLT16_MIN_EXP __FLT16_MIN_EXP__
#define FLT16_MIN_10_EXP __FLT16_MIN_10_EXP__
#define FLT16_MAX_EXP __FLT16_MAX_EXP__
#define FLT16_MAX_10_EXP __FLT16_MAX_10_EXP__
#define FLT16_MAX ((_Float16)__FLT16_MAX__)
#define FLT16_EPSILON ((_Float16)__FLT16_EPSILON__)
#define FLT16_MIN ((_Float16)__FLT16_MIN__)
#define FLT16_TRUE_MIN ((_Float16)__FLT16_DENORM_MIN__)
#define FLT16_HAS_SUBNORM __FLT16_HAS_DENORM__
#endif
#endif

#endif
