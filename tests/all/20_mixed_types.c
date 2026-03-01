float average(int a, int b, int c) {
    float sum = (float)(a + b + c);
    float avg = sum / 3.0f;
    return avg;
}

int truncate(double x) {
    return (int)x;
}

double scale(int x, double factor) {
    return (double)x * factor;
}
