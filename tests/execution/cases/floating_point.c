static float scale_and_offset(float value, float scale, float offset) {
    return value * scale + offset;
}

static double average(double left, double right) {
    return (left + right) / 2.0;
}

static double combine(float left, double right) {
    return (double)left + right;
}

int main(void) {
    float single = 1.5f;
    double wide = 6.25;
    float single_result = scale_and_offset(single, 2.0f, 1.0f);
    double wide_result = average(wide, 9.75);
    double mixed_result = combine(2.5f, 3.5);
    int truncated = (int)3.75;
    double converted = (double)6;

    if (single_result != 4.0f || single_result <= single)
        return 1;
    if (wide_result != 8.0 || wide_result < wide)
        return 2;
    if (mixed_result != 6.0 || mixed_result == wide_result)
        return 3;
    if (truncated != 3 || converted != 6.0)
        return 4;
    return 51;
}
