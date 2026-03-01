// Test: Selection Sort
void selection_sort(int n) {
    int arr[8];
    arr[0] = 64;
    arr[1] = 25;
    arr[2] = 12;
    arr[3] = 22;
    arr[4] = 11;
    arr[5] = 90;
    arr[6] = 3;
    arr[7] = 47;

    int i = 0;
    while (i < n - 1) {
        int min_idx = i;
        int j = i + 1;
        while (j < n) {
            if (arr[j] < arr[min_idx]) {
                min_idx = j;
            }
            j = j + 1;
        }
        int temp = arr[min_idx];
        arr[min_idx] = arr[i];
        arr[i] = temp;
        i = i + 1;
    }
}
