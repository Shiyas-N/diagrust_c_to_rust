// Test: Linear Search
int linear_search(int n, int target) {
    int arr[5];
    arr[0] = 10;
    arr[1] = 20;
    arr[2] = 30;
    arr[3] = 40;
    arr[4] = 50;

    int i = 0;
    while (i < n) {
        if (arr[i] == target) {
            return i;
        }
        i = i + 1;
    }
    return -1;
}
