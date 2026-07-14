extern int values[];
extern int values[3];

static int state = 1;

int transform();
int transform(int value);

int transform(int value) {
    return value + state;
}

int drive(int limit) {
    int total = 0;
    int (*operation)(int) = transform;

    for (int index = 0; index < limit; ++index) {
        switch (index & 1) {
        case 0:
            total += values[index];
            break;
        default:
            total += index;
            if (total > 20)
                goto done;
        }
    }

done:
    return operation(total);
}

int values[3] = { 2, 3, 5 };
