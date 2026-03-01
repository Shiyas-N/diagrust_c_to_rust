int sum_to_n(int n) {
    int sum = 0;
    int i = 1;
    while (i <= n) {
        sum = sum + i;
        i = i + 1;
    }
    return sum;
}

int count_digits(int n) {
    int count = 0;
    while (n > 0) {
        n = n / 10;
        count = count + 1;
    }
    return count;
}
