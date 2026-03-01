int factorial(int n) {
    int result = 1;
    int i = 1;
    for (i = 1; i <= n; i++) {
        result = result * i;
    }
    return result;
}

int power(int base, int exp) {
    int result = 1;
    int i = 0;
    for (i = 0; i < exp; i++) {
        result = result * base;
    }
    return result;
}
