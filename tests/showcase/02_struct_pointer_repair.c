// Showcase: Struct & Pointer Repair
// Triggers: ImmutableRef (E0594), AddMut (E0596), LoopVarMut,
//           CompoundAssign, struct field access, array swap
// The repair loop must upgrade & to &mut, add mut to bindings,
// and handle struct field modification through pointers.
#include <stdio.h>

struct Rectangle {
    int width;
    int height;
};

int area(struct Rectangle *r) {
    return r->width * r->height;
}

void scale(struct Rectangle *r, int factor) {
    r->width = r->width * factor;
    r->height = r->height * factor;
}

void swap(int arr[], int i, int j) {
    int temp = arr[i];
    arr[i] = arr[j];
    arr[j] = temp;
}

void insertion_sort(int arr[], int n) {
    int i, j;
    for (i = 1; i < n; i++) {
        int key = arr[i];
        j = i - 1;
        while (j >= 0 && arr[j] > key) {
            arr[j + 1] = arr[j];
            j = j - 1;
        }
        arr[j + 1] = key;
    }
}

int main() {
    struct Rectangle rect;
    rect.width = 5;
    rect.height = 10;

    printf("Area: %d\n", area(&rect));

    scale(&rect, 2);
    printf("Scaled area: %d\n", area(&rect));
    printf("Width: %d, Height: %d\n", rect.width, rect.height);

    // Array sorting with mutation
    int arr[6];
    arr[0] = 64;
    arr[1] = 25;
    arr[2] = 12;
    arr[3] = 22;
    arr[4] = 11;
    arr[5] = 90;

    insertion_sort(arr, 6);

    printf("Sorted: ");
    int i;
    for (i = 0; i < 6; i++) {
        printf("%d ", arr[i]);
    }
    printf("\n");

    return 0;
}
