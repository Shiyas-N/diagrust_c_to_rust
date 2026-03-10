// Test: switch/case with multiple cases and default
#include <stdio.h>

int grade_to_gpa(int grade) {
    int gpa;
    switch(grade) {
        case 4:
            gpa = 10;
            break;
        case 3:
            gpa = 8;
            break;
        case 2:
            gpa = 6;
            break;
        case 1:
            gpa = 4;
            break;
        default:
            gpa = 0;
            break;
    }
    return gpa;
}

int main() {
    printf("Grade 4 GPA: %d\n", grade_to_gpa(4));
    printf("Grade 2 GPA: %d\n", grade_to_gpa(2));
    printf("Grade 0 GPA: %d\n", grade_to_gpa(0));
    return 0;
}
