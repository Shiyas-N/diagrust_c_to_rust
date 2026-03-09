#include <stdio.h>

int sum_array(int arr[], int n) {
    int sum = 0;
    int i = 0;
    while (i < n) {
        sum = sum + arr[i];
        i = i + 1;
    }
    return sum;
}

int find_max(int arr[], int n) {
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

int main() {
    int arr[5];
    arr[0] = 10;
    arr[1] = 25;
    arr[2] = 7;
    arr[3] = 42;
    arr[4] = 13;

    int n = 5;
    int total = sum_array(arr, n);
    int maximum = find_max(arr, n);

    printf("Array: ");
    int i = 0;
    while (i < n) {
        printf("%d ", arr[i]);
        i = i + 1;
    }
    printf("\n");
    printf("Sum: %d\n", total);
    printf("Max: %d\n", maximum);

    return 0;
}
