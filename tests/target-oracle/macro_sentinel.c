#if __SIZEOF_POINTER__ != 8 || __SIZEOF_LONG__ != 8
#error target is not LP64
#endif

#if defined(__aarch64__) && defined(__linux__)
#if !defined(__ARM_ARCH_8A) || __ARM_ARCH != 8 || __ARM_PCS_AAPCS64 != 1
#error unexpected AArch64 GNU profile
#endif
#if !defined(__CHAR_UNSIGNED__) || __LDBL_MANT_DIG__ != 113
#error unexpected AArch64 data model
#endif
#elif defined(__riscv) && defined(__linux__)
#if __riscv_xlen != 64 || __riscv_float_abi_double != 1
#error unexpected RISC-V data model
#endif
#if !defined(__riscv_cmodel_medany) || !defined(__riscv_cmodel_pic)
#error unexpected RISC-V PIE code model
#endif
#if !defined(__riscv_zicsr) || !defined(__riscv_zifencei)
#error incomplete RV64GC compatibility identity
#endif
#elif defined(__x86_64__) && defined(__linux__)
#if !defined(__amd64__) || __LDBL_MANT_DIG__ != 64
#error unexpected x86-64 GNU data model
#endif
#if __SIZEOF_LONG_DOUBLE__ != 16
#error unexpected x86-64 long-double object size
#endif
#elif defined(__APPLE__) && defined(__arm64__)
#if __APPLE_CC__ != 6000 || __LDBL_MANT_DIG__ != 53
#error unexpected Apple compiler or long-double profile
#endif
#if __ENVIRONMENT_MAC_OS_X_VERSION_MIN_REQUIRED__ != 110000
#error unexpected macOS deployment identity
#endif
#else
#error target sentinel reached an unknown profile
#endif

int macro_sentinel(void) { return 0; }
