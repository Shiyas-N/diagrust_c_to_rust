// Test: Prefix sum array
void prefix_sum(int n) {
    int arr[6];
    arr[0] = 3;
    arr[1] = 1;
    arr[2] = 4;
    arr[3] = 1;
    arr[4] = 5;
    arr[5] = 9;

    int prefix[6];
    prefix[0] = arr[0];

    int i = 1;
    while (i < n) {
        prefix[i] = prefix[i - 1] + arr[i];
        i = i + 1;
    }
}

// Count elements greater than threshold
int count_above(int n, int threshold) {
    int arr[7];
    arr[0] = 10;
    arr[1] = 3;
    arr[2] = 25;
    arr[3] = 8;
    arr[4] = 17;
    arr[5] = 2;
    arr[6] = 30;

    int count = 0;
    int i = 0;
    while (i < n) {
        if (arr[i] > threshold) {
            count = count + 1;
        }
        i = i + 1;
    }
    return count;
}
