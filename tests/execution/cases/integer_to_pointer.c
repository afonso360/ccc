int main(void) {
    void *signed_value = (void *)(-1);
    void *unsigned_value = (void *)(~0U);

    if ((unsigned long)signed_value != ~0UL)
        return 1;
    if ((unsigned long)unsigned_value != (unsigned long)~0U)
        return 2;
    return 55;
}
