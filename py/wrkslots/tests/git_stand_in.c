/* The wrkslots tests' git stand-in (#2065; see git_stand_in.py, which compiles this file once
 * per test session with -DREAL_GIT and -DCALL_LOG).
 *
 * The real git runs in its own unprivileged user and PID namespace under a small init (PID 1
 * of that namespace). When git exits, the init exits, and the kernel kills whatever git left
 * behind before this process sees the init's exit, so nothing outlives the call. Exit codes,
 * signal deaths and termination signals sent to this process pass through. Where the host
 * refuses unprivileged namespaces it runs plain git. Each call appends "ns 0" or
 * "plain <errno>" to CALL_LOG. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <sched.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/wait.h>
#include <unistd.h>

static volatile pid_t target;

static void forward(int sig) {
    if (target > 0) kill(target, sig);
}

static void forward_signals(void) {
    static const int sigs[] = {SIGTERM, SIGINT, SIGHUP, SIGQUIT};
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_handler = forward;
    sigemptyset(&action.sa_mask);
    for (size_t i = 0; i < sizeof sigs / sizeof sigs[0]; i++) sigaction(sigs[i], &action, NULL);
}

static int write_file(const char *path, const char *text) {
    int fd = open(path, O_WRONLY | O_CLOEXEC);
    if (fd < 0) return -1;
    size_t len = strlen(text);
    ssize_t n = write(fd, text, len);
    int closed = close(fd);
    return (n == (ssize_t)len && closed == 0) ? 0 : -1;
}

static void note_call(const char *how) {
    int saved = errno;
    const char *log = CALL_LOG; /* baked in: counted even when a test passes git a scrubbed environment */
    int fd = open(log, O_WRONLY | O_APPEND | O_CREAT | O_CLOEXEC, 0644);
    if (fd < 0) return;
    char line[64];
    int len = snprintf(line, sizeof line, "%s %d\n", how, saved);
    if (len > 0 && write(fd, line, (size_t)len) < 0) { /* best effort */ }
    close(fd);
}

static int wait_for(pid_t pid) {
    int status = 0;
    while (waitpid(pid, &status, 0) < 0) {
        if (errno != EINTR) return -1;
    }
    return status;
}

int main(int argc, char **argv) {
    (void)argc;
    const char *real = REAL_GIT; /* baked in when the test session compiles this file */
    argv[0] = (char *)real;
    unsigned uid = (unsigned)getuid(), gid = (unsigned)getgid();
    if (unshare(CLONE_NEWUSER | CLONE_NEWPID) != 0) {
        note_call("plain");
        execv(real, argv); /* namespaces refused here: plain git */
        perror("git stand-in: exec");
        return 127;
    }
    char map[64];
    snprintf(map, sizeof map, "%u %u 1", uid, uid);
    int failed = write_file("/proc/self/uid_map", map);
    failed = failed || write_file("/proc/self/setgroups", "deny");
    snprintf(map, sizeof map, "%u %u 1", gid, gid);
    failed = failed || write_file("/proc/self/gid_map", map);
    if (failed) {
        perror("git stand-in: cannot map this user into its namespace");
        return 127;
    }
    errno = 0;
    note_call("ns");
    int note_pipe[2];
    if (pipe2(note_pipe, O_CLOEXEC) != 0) {
        perror("git stand-in: pipe");
        return 127;
    }
    pid_t init = fork();
    if (init < 0) {
        perror("git stand-in: fork");
        return 127;
    }
    if (init == 0) { /* PID 1 of the new namespace */
        close(note_pipe[0]);
        prctl(PR_SET_PDEATHSIG, SIGKILL); /* the namespace dies with this call */
        pid_t child = fork();
        if (child == 0) {
            execv(real, argv);
            perror("git stand-in: exec");
            _exit(127);
        }
        if (child < 0) _exit(127);
        target = child;
        forward_signals();
        int status = wait_for(child);
        unsigned char note[2] = {'E', 127};
        if (status >= 0 && WIFSIGNALED(status)) {
            note[0] = 'S';
            note[1] = (unsigned char)WTERMSIG(status);
        } else if (status >= 0) {
            note[1] = (unsigned char)WEXITSTATUS(status);
        }
        if (write(note_pipe[1], note, 2) != 2) _exit(127);
        _exit(0); /* the kernel kills what is left in the namespace before this exit is reported */
    }
    close(note_pipe[1]);
    target = init;
    forward_signals();
    int status = wait_for(init);
    unsigned char note[2];
    ssize_t n;
    while ((n = read(note_pipe[0], note, 2)) < 0 && errno == EINTR) {
    }
    if (n != 2) { /* the init itself was killed */
        if (status >= 0 && WIFSIGNALED(status)) {
            signal(WTERMSIG(status), SIG_DFL);
            kill(getpid(), WTERMSIG(status));
        }
        return 127;
    }
    if (note[0] == 'S') {
        signal(note[1], SIG_DFL);
        sigset_t set;
        sigemptyset(&set);
        sigaddset(&set, note[1]);
        sigprocmask(SIG_UNBLOCK, &set, NULL);
        kill(getpid(), note[1]);
        return 128 + note[1];
    }
    return note[1];
}
