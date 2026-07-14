int main(void) {
    int total = 0;
    int index;
    int pass = 0;

    for (index = 0; index < 6; index = index + 1) {
        if (index == 1)
            continue;
        if (index == 5)
            break;
        total = total + index;
    }

    do {
        pass = pass + 1;
        if (pass == 2)
            goto after_loop;
        total = total + 10;
    } while (pass < 4);

after_loop:
    switch (total) {
    case 19:
        total = total + 20;
        break;
    default:
        return 1;
    }

    switch (pass) {
    case 2:
        total = total + 7;
        /* Fall through to exercise ordered case execution. */
    case 3:
        total = total + 1;
        break;
    default:
        return 2;
    }
    return total;
}
