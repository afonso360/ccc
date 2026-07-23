#ifndef CCC_TESTFLOAT_SUBJFLOAT_CONFIG_H
#define CCC_TESTFLOAT_SUBJFLOAT_CONFIG_H

/* Keep this list equal to the operations exercised by run-testfloat.sh. */
#define SUBJ_UI64_TO_EXTF80
#define SUBJ_I64_TO_EXTF80
#define SUBJ_F32_TO_EXTF80
#define SUBJ_F64_TO_EXTF80

/* Representable C casts truncate toward zero and retain x87 inexact.  The
 * adapter handles NaN and out-of-range inputs before the cast, returning the
 * pinned 8086-SSE specialization's integer-indefinite values and raising the
 * invalid exception explicitly. */
#define SUBJ_EXTF80_TO_UI64_RX_MINMAG
#define SUBJ_EXTF80_TO_I64_RX_MINMAG
#define SUBJ_EXTF80_TO_F32
#define SUBJ_EXTF80_TO_F64

#define SUBJ_EXTF80_ADD
#define SUBJ_EXTF80_SUB
#define SUBJ_EXTF80_MUL
#define SUBJ_EXTF80_DIV

/* C equality is quiet for a quiet NaN; ordered relations are signaling. */
#define SUBJ_EXTF80_EQ
#define SUBJ_EXTF80_LE
#define SUBJ_EXTF80_LT

#endif
