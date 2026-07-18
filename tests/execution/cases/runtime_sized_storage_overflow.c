int main(void) {
    volatile unsigned long count = 0x4000000000000000UL;
    int values[count];
    values[0] = 1;
    return values[0];
}
