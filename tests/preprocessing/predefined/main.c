#if __STDC__ != 1
#error __STDC__ has an unexpected value
#endif

#if __STDC_VERSION__ != 201112L
#error __STDC_VERSION__ has an unexpected value
#endif

#if !defined(__CCC__) || !defined(__GNUC__)
#error compiler compatibility macros are missing
#endif

int main(void) {
    return __SIZEOF_POINTER__ == 8 ? 0 : 1;
}
