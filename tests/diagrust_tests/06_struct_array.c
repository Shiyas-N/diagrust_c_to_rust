#include <stdio.h>
#include <stdlib.h>

struct Student {
    int id;
    int age;
    float grade;
};

struct Student create_student(int id, int age, float grade) {
    struct Student s;
    s.id = id;
    s.age = age;
    s.grade = grade;
    return s;
}

void print_student(struct Student s) {
    printf("ID: %d  Age: %d  Grade: %.1f\n", s.id, s.age, s.grade);
}

float average_grade(struct Student arr[], int n) {
    float total = 0;
    int i = 0;
    while (i < n) {
        total = total + arr[i].grade;
        i = i + 1;
    }
    return total / n;
}

int main() {
    struct Student students[3];
    students[0] = create_student(1, 20, 88.5);
    students[1] = create_student(2, 21, 92.0);
    students[2] = create_student(3, 19, 76.5);

    printf("Student Records:\n");
    int i = 0;
    while (i < 3) {
        print_student(students[i]);
        i = i + 1;
    }

    float avg = average_grade(students, 3);
    printf("Average grade: %.1f\n", avg);

    return 0;
}
