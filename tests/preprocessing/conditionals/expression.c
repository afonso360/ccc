#define VALUE 40

#if defined(VALUE) && VALUE == 40 && !defined(MISSING)
#define RESULT 42
#elif VALUE > 0
#define RESULT 1
#else
#define RESULT 0
#endif

int main(void) {
    return RESULT;
}
