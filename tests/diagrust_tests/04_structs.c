#include <stdio.h>

struct Point {
    int x;
    int y;
};

struct Rectangle {
    struct Point top_left;
    struct Point bottom_right;
};

struct Point make_point(int x, int y) {
    struct Point p;
    p.x = x;
    p.y = y;
    return p;
}

int rect_width(struct Rectangle r) {
    return r.bottom_right.x - r.top_left.x;
}

int rect_height(struct Rectangle r) {
    return r.bottom_right.y - r.top_left.y;
}

int rect_area(struct Rectangle r) {
    return rect_width(r) * rect_height(r);
}

void print_point(struct Point p) {
    printf("(%d, %d)", p.x, p.y);
}

int main() {
    struct Point a;
    a.x = 3;
    a.y = 4;

    struct Point b;
    b.x = 7;
    b.y = 9;

    printf("Point A: ");
    print_point(a);
    printf("\n");

    printf("Point B: ");
    print_point(b);
    printf("\n");

    struct Rectangle rect;
    rect.top_left = make_point(0, 0);
    rect.bottom_right = make_point(10, 5);

    printf("Rectangle width:  %d\n", rect_width(rect));
    printf("Rectangle height: %d\n", rect_height(rect));
    printf("Rectangle area:   %d\n", rect_area(rect));

    return 0;
}
