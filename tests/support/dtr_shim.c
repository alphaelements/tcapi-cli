/* LD_PRELOAD shim for the simulator tests.
 *
 * The SDK asserts DTR when it opens a serial port by path
 * (TIOCMBIS on the modem lines). A pseudo terminal has no modem lines and
 * answers ENOTTY, so the real `tcapi` binary could not open the
 * simulator's pty. This shim turns that one failure into success for the
 * four modem-line requests; every other ioctl, and every other error, is
 * passed through untouched. It is loaded only into the test processes
 * that talk to `tcapi-dio-sim`. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdarg.h>
#include <sys/ioctl.h>

int ioctl(int fd, unsigned long request, ...)
{
    static int (*real)(int, unsigned long, ...);
    va_list args;
    void *arg;
    int result;

    if (!real) {
        real = (int (*)(int, unsigned long, ...))dlsym(RTLD_NEXT, "ioctl");
    }
    va_start(args, request);
    arg = va_arg(args, void *);
    va_end(args);

    result = real(fd, request, arg);
    if (result == -1 && errno == ENOTTY &&
        (request == TIOCMBIS || request == TIOCMBIC ||
         request == TIOCMSET || request == TIOCMGET)) {
        return 0;
    }
    return result;
}
