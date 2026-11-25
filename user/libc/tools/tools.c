/* SPDX-License-Identifier: GPL-3.0-only */
/* The small tools built into Alaya's terminal (milestone 8, step 4),
 * written as ordinary C programs on Alaya's C library: each is a main
 * function the terminal runs with alaya_run_main. */
#include <ctype.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "../../apps/include/alaya.h"

int tool_hello(int argc, char **argv) {
    (void)argc;
    (void)argv;
    printf("Hello from C, on Alaya.\n");
    return 0;
}

int tool_echo(int argc, char **argv) {
    for (int i = 1; i < argc; i++)
        printf("%s%s", argv[i], i + 1 < argc ? " " : "");
    putchar('\n');
    return 0;
}

int tool_cat(int argc, char **argv) {
    int status = 0;
    for (int i = 1; i < argc; i++) {
        FILE *f = fopen(argv[i], "r");
        if (!f) {
            fprintf(stderr, "cat: %s: %s\n", argv[i], strerror(errno));
            status = 1;
            continue;
        }
        char buf[256];
        size_t n;
        while ((n = fread(buf, 1, sizeof buf, f)) > 0)
            fwrite(buf, 1, n, stdout);
        fclose(f);
    }
    return status;
}

int tool_wc(int argc, char **argv) {
    int status = 0;
    long tl = 0, tw = 0, tc = 0;
    for (int i = 1; i < argc; i++) {
        FILE *f = fopen(argv[i], "r");
        if (!f) {
            fprintf(stderr, "wc: %s: %s\n", argv[i], strerror(errno));
            status = 1;
            continue;
        }
        long lines = 0, words = 0, chars = 0;
        int in_word = 0, c;
        while ((c = fgetc(f)) != EOF) {
            chars++;
            if (c == '\n')
                lines++;
            if (isspace(c)) {
                in_word = 0;
            } else if (!in_word) {
                in_word = 1;
                words++;
            }
        }
        fclose(f);
        printf("%7ld %7ld %7ld %s\n", lines, words, chars, argv[i]);
        tl += lines;
        tw += words;
        tc += chars;
    }
    if (argc > 2)
        printf("%7ld %7ld %7ld total\n", tl, tw, tc);
    return status;
}

int tool_ls(int argc, char **argv) {
    (void)argc;
    (void)argv;
    uint64_t docs;
    if (alaya_store_root(ALAYA_DOCUMENTS, &docs)) {
        fprintf(stderr, "ls: no documents\n");
        return 1;
    }
    static uint8_t buf[4096];
    size_t got = 0;
    if (alaya_store_list(docs, buf, sizeof buf - 1, &got)) {
        fprintf(stderr, "ls: the documents could not be listed\n");
        return 1;
    }
    buf[got] = 0;
    if (!got)
        printf("(nothing handed to the terminal yet: try \"pick\")\n");
    else
        fputs((char *)buf, stdout);
    return 0;
}

/* Copy standard input... Alaya has none: copy one file to another. */
int tool_cp(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: cp FROM TO\n");
        return 2;
    }
    FILE *in = fopen(argv[1], "r");
    if (!in) {
        perror(argv[1]);
        return 1;
    }
    FILE *out = fopen(argv[2], "w");
    if (!out) {
        perror(argv[2]);
        fclose(in);
        return 1;
    }
    char buf[256];
    size_t n;
    while ((n = fread(buf, 1, sizeof buf, in)) > 0)
        fwrite(buf, 1, n, out);
    fclose(in);
    return fclose(out) ? 1 : 0;
}

static int cmp_int(const void *a, const void *b) {
    int x = *(const int *)a, y = *(const int *)b;
    return (x > y) - (x < y);
}

/* The C library checks itself: formats, strings, numbers, sorting, a
 * file written and read back, and exit() from deep inside. Prints one
 * line per failure, then "selftest: N of M passed". */
static int passed, total;
static void check(int ok, const char *what) {
    total++;
    if (ok)
        passed++;
    else
        printf("FAILED: %s\n", what);
}

static void deep(int n) {
    if (n == 0)
        exit(7);
    deep(n - 1);
}

int tool_exit_deep(int argc, char **argv) {
    (void)argc;
    (void)argv;
    deep(20);
    return 0;
}

int alaya_run_main(int (*main_fn)(int, char **), int argc, char **argv);

int tool_selftest(int argc, char **argv) {
    (void)argc;
    (void)argv;
    char b[128];
    passed = total = 0;
    snprintf(b, sizeof b, "%d|%5d|%-5d|%05d|%+d|% d", 42, -42, 7, 31, 3, 4);
    check(!strcmp(b, "42|  -42|7    |00031|+3| 4"), "integers");
    snprintf(b, sizeof b, "%x|%X|%#x|%o|%#o|%u", 255u, 255u, 255u, 8u, 8u, 4000000000u);
    check(!strcmp(b, "ff|FF|0xff|10|010|4000000000"), "hex, octal, unsigned");
    snprintf(b, sizeof b, "%lld|%llu|%ld|%zu|%hhd|%hd", -9000000000LL, 18000000000ULL, -5L,
             (size_t)12, (signed char)-3, (short)-300);
    check(!strcmp(b, "-9000000000|18000000000|-5|12|-3|-300"), "lengths");
    snprintf(b, sizeof b, "[%s|%.3s|%8s|%-8s|%c|%%]", "alaya", "alaya", "os", "os", 'x');
    check(!strcmp(b, "[alaya|ala|      os|os      |x|%]"), "strings and chars");
    snprintf(b, sizeof b, "%.5d|%8.3d|%*d|%-*d|", 42, 7, 4, 9, 3, 1);
    check(!strcmp(b, "00042|     007|   9|1  |"), "precision and *");
    check(snprintf(b, 4, "abcdef") == 6 && !strcmp(b, "abc"), "snprintf cuts and counts");
    check(strlen("hello") == 5 && strcmp("a", "b") < 0 && strncmp("abc", "abd", 2) == 0, "strlen, strcmp");
    check(strchr("hello", 'l') - "hello" == 2 && strrchr("hello", 'l') - "hello" == 3, "strchr");
    check(strstr("the alaya os", "alaya") != NULL && !strstr("abc", "x"), "strstr");
    strcpy(b, "one");
    strcat(b, ",two");
    check(!strcmp(b, "one,two") && strspn("aab", "a") == 2 && strcspn("ab,c", ",") == 2, "strcat, spans");
    char t[] = "a, b,,c";
    char *x = strtok(t, ", ");
    char *y = strtok(NULL, ", ");
    char *z = strtok(NULL, ", ");
    check(x && y && z && !strcmp(x, "a") && !strcmp(y, "b") && !strcmp(z, "c") && !strtok(NULL, ", "), "strtok");
    char *end;
    check(strtol("  -123abc", &end, 10) == -123 && *end == 'a', "strtol");
    check(strtol("0x1f", NULL, 0) == 31 && strtol("017", NULL, 0) == 15 && atoi("99") == 99, "bases");
    check(strtoull("18446744073709551615", NULL, 10) == 18446744073709551615ULL, "strtoull");
    check(toupper('a') == 'A' && isdigit('7') && !isalpha('7') && isspace('\t'), "ctype");
    int v[] = {5, 3, 9, 1, 7, 2, 8, 6, 4, 0};
    qsort(v, 10, sizeof v[0], cmp_int);
    int sorted = 1;
    for (int i = 0; i < 10; i++)
        sorted &= v[i] == i;
    int key = 6;
    check(sorted && bsearch(&key, v, 10, sizeof v[0], cmp_int) == &v[6], "qsort, bsearch");
    char *d = strdup("copied");
    char *m = calloc(16, 4);
    int zero = 1;
    for (int i = 0; i < 64; i++)
        zero &= m[i] == 0;
    m = realloc(m, 4096);
    m[4095] = 1;
    check(d && !strcmp(d, "copied") && zero && m, "malloc family");
    free(d);
    free(m);
    FILE *f = fopen("/app/selftest.txt", "w");
    check(f != NULL, "a file of its own");
    if (f) {
        fprintf(f, "line %d\nline %d\n", 1, 2);
        fclose(f);
        f = fopen("/app/selftest.txt", "r");
        char l1[32], l2[32];
        check(f && fgets(l1, sizeof l1, f) && fgets(l2, sizeof l2, f) && !fgets(l2, sizeof l2, f) &&
                  !strcmp(l1, "line 1\n") && !strcmp(l2, "line 2\n") && feof(f),
              "written and read back");
        if (f)
            fclose(f);
        check(remove("/app/selftest.txt") == 0 && !fopen("/app/selftest.txt", "r") && errno == ENOENT,
              "removed");
    }
    check(!fopen("/etc/passwd", "r") && errno == ENOENT, "no other paths");
    check(alaya_run_main(tool_exit_deep, 1, argv) == 7, "exit from deep inside");
    printf("selftest: %d of %d passed\n", passed, total);
    return passed == total ? 0 : 1;
}
