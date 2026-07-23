#include <setjmp.h>

static jmp_buf environment;

static void resume_inner(int value) {
    longjmp(environment, value);
}

static void resume_saved_environment(int value) {
    resume_inner(value);
}

int main(void) {
    int unchanged = 7;
    volatile int modified = 1;
    int result = setjmp(environment);
    if (result != 0) {
        if (result != 23)
            return 1;
        if (unchanged != 7)
            return 2;
        if (modified != 9)
            return 3;
        return 0;
    }

    modified = 9;
    resume_saved_environment(23);
    return 4;
}
