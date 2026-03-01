// Test: Sum and Average of array
int array_sum(int n) {
    int arr[6];
    arr[0] = 5;
    arr[1] = 10;
    arr[2] = 15;
    arr[3] = 20;
    arr[4] = 25;
    arr[5] = 30;

    int sum = 0;
    int i = 0;
    while (i < n) {
        sum = sum + arr[i];
        i = i + 1;
    }
    return sum;
}

int find_max(int n) {
    int arr[5];
    arr[0] = 3;
    arr[1] = 7;
    arr[2] = 1;
    arr[3] = 9;
    arr[4] = 4;

    int max = arr[0];
    int i = 1;
    while (i < n) {
        if (arr[i] > max) {
            max = arr[i];
        }
        i = i + 1;
    }
    return max;
}

int find_min(int n) {
    int arr[5];
    arr[0] = 3;
    arr[1] = 7;
    arr[2] = 1;
    arr[3] = 9;
    arr[4] = 4;

    int min = arr[0];
    int i = 1;
    while (i < n) {
        if (arr[i] < min) {
            min = arr[i];
        }
        i = i + 1;
    }
    return min;
}
