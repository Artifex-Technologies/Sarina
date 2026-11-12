/* For the side-by-side checks only (tests/parity.py loads it with LD_PRELOAD): the wall clock reads a fixed time.
 *
 * ONNX Runtime seeds the random numbers inside a graph (the decoder's noise) from the wall clock when the library is loaded,
 * and neither the Python service nor the C API sets that seed (Python's onnxruntime.set_seed exists, the C API's SetSeed is
 * only in training builds). With the same fixed clock, the Python and the Rust process start from the same seed, so their
 * decoders draw the same noise and the audio can be compared bit for bit. Monotonic clocks are left alone. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdlib.h>
#include <time.h>

int clock_gettime(clockid_t id, struct timespec *ts) {
    static int (*real)(clockid_t, struct timespec *);
    if (!real)
        real = (int (*)(clockid_t, struct timespec *))dlsym(RTLD_NEXT, "clock_gettime");
    const char *s = getenv("FIXED_REALTIME_NS");
    if (id == CLOCK_REALTIME && s) {
        long long ns = atoll(s);
        ts->tv_sec = ns / 1000000000LL;
        ts->tv_nsec = ns % 1000000000LL;
        return 0;
    }
    return real(id, ts);
}
