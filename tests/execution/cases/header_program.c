#include "headers/arithmetic.h"

int identity(int value) {
    return value;
}

int main(void) {
    return ADD_TWO(identity(BASE_VALUE));
}
