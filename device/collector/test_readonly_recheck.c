/* Exact production scheduler with a synthetic monotonic clock; no network. */
#define _GNU_SOURCE
#include <sys/stat.h>
#include <time.h>
int recheck_test_clock(clockid_t, struct timespec *);
int recheck_test_fstat(int, struct stat *);
#define clock_gettime recheck_test_clock
#define fstat recheck_test_fstat
#define main collector_program_main
#include "ksync-collector.c"
#undef main
#undef fstat
#undef clock_gettime
#include <assert.h>
static long long test_ms = 100000;
static int wrong_marker_owner;
int recheck_test_clock(clockid_t clock, struct timespec *t) {
  assert(clock == CLOCK_MONOTONIC);
  t->tv_sec = test_ms / 1000;
  t->tv_nsec = (test_ms % 1000) * 1000000;
  return 0;
}
int recheck_test_fstat(int fd, struct stat *s) {
  int result = fstat(fd, s);
  if (!result && wrong_marker_owner && S_ISREG(s->st_mode))
    s->st_uid = geteuid() + 1;
  return result;
}
static void marker(const char *path, mode_t mode, int bytes) {
  int fd = open(path, O_WRONLY | O_CREAT | O_EXCL, mode);
  assert(fd >= 0 && fchmod(fd, mode) == 0);
  if (bytes) assert(write(fd, "x", 1) == 1);
  close(fd);
}
int main(int argc, char **argv) {
  assert(argc == 2);
  snprintf(root, sizeof root, "%s", argv[1]);
  char pending[PATH_CAP], outside[PATH_CAP], events[PATH_CAP], bin[PATH_CAP], exe[PATH_CAP];
  join(pending, root, "readonly-recheck.pending");
  join(outside, root, "outside-fixture");
  marker(outside, 0600, 0);
  marker(pending, 0600, 1);
  assert(!consume_readonly_recheck() && !readonly_recheck_due_ms);
  assert(access(pending, F_OK) == 0); assert(unlink(pending) == 0);
  marker(pending, 0640, 0);
  assert(!consume_readonly_recheck()); assert(unlink(pending) == 0);
  assert(symlink(outside, pending) == 0);
  assert(!consume_readonly_recheck());
  struct stat st; assert(lstat(pending, &st) == 0 && S_ISLNK(st.st_mode));
  assert(unlink(pending) == 0 && access(outside, F_OK) == 0);
  assert(link(outside, pending) == 0);
  assert(!consume_readonly_recheck()); assert(unlink(pending) == 0);
  marker(pending, 0600, 0); wrong_marker_owner = 1;
  assert(!consume_readonly_recheck()); wrong_marker_owner = 0;
  assert(access(pending, F_OK) == 0);
  assert(consume_readonly_recheck());
  assert(access(pending, F_OK) == -1 && readonly_recheck_due_ms == test_ms + 60000);
  strcpy(reader, "idle"); strcpy(power, "awake"); net = 1;
  idle_mono_ms = test_ms - 30000; strcpy(last_worker_mode, "sync");
  assert(readonly_recheck_timeout(test_ms) == 60000);
  test_ms += 60000;
  strcpy(reader, "active"); assert(readonly_recheck_timeout(test_ms) == -1);
  maybe_worker(); assert(!worker && readonly_recheck_due_ms);
  strcpy(reader, "idle"); strcpy(power, "sleeping");
  assert(readonly_recheck_timeout(test_ms) == -1); maybe_worker(); assert(!worker);
  strcpy(power, "awake"); usb = 1;
  assert(readonly_recheck_timeout(test_ms) == -1); maybe_worker(); assert(!worker);
  usb = 0; blocked = 1;
  assert(readonly_recheck_timeout(test_ms) == -1); maybe_worker(); assert(!worker);
  blocked = 0; net = 0; strcpy(last_worker_mode, "collect");
  assert(readonly_recheck_timeout(test_ms) == -1); maybe_worker(); assert(!worker);
  net = 1; strcpy(last_worker_mode, "sync"); idle_mono_ms = test_ms - 19000;
  assert(readonly_recheck_timeout(test_ms) == -1); maybe_worker(); assert(!worker);
  idle_mono_ms -= 1000;
  assert(readonly_recheck_timeout(test_ms) == 0);
  join(events, root, "events"); assert(mkdir_private(events) == 0);
  join(sealedpath, events, "sealed"); assert(mkdir_private(sealedpath) == 0);
  join(activepath, events, "active.log"); join(checkpoint, root, "collector.checkpoint");
  join(runtimepath, root, "runtime.json"); join(bin, root, "bin");
  assert(mkdir_private(bin) == 0); join(exe, bin, "ksync-worker");
  FILE *script = fopen(exe, "w"); assert(script);
  fprintf(script, "#!/bin/sh\nprintf '%%s\\n' \"$3\" >> \"%s/worker-calls\"\n", root);
  fclose(script); assert(chmod(exe, 0700) == 0);
  maybe_worker(); assert(worker > 0 && !readonly_recheck_due_ms);
  assert(waitpid(worker, NULL, 0) == worker); worker = 0;
  char calls[PATH_CAP]; join(calls, root, "worker-calls");
  FILE *read = fopen(calls, "r"); char body[32] = {0}; assert(read);
  assert(fread(body, 1, sizeof body - 1, read) == 5); fclose(read);
  assert(!strcmp(body, "sync\n"));
  test_ms += 120000; maybe_worker(); assert(!worker && !readonly_recheck_due_ms);
  /* A later worker exit consumes a new hint and schedules exactly one future
   * pass. No recurring timer is created by an exit with no new hint. */
  worker = fork(); assert(worker >= 0);
  if (!worker) { marker(pending, 0600, 0); _exit(0); }
  for (int i = 0; worker > 0 && i < 1000; i++) { usleep(1000); maybe_worker(); }
  assert(!worker && readonly_recheck_due_ms == test_ms + 60000);
  assert(access(pending, F_OK) == -1);
  /* A manual worker can finish after our previous child exit. Its explicit
   * nudge consumes only the hint, without acknowledging gaps or clearing any
   * safety/reader state, and an absent hint cannot create a periodic timer. */
  readonly_recheck_due_ms = 0; marker(pending, 0600, 0);
  strcpy(reader, "active"); strcpy(power, "sleeping");
  net = 0; usb = 1; blocked = 1; coverage_gap = 1; dirty = 0; room_checked = 123;
  strcpy(last_worker_mode, "sync");
  char nudge[] = "maintenance check-readonly-hint";
  control_line(nudge);
  assert(readonly_recheck_due_ms == test_ms + 60000);
  assert(access(pending, F_OK) == -1);
  assert(!strcmp(reader, "active") && !strcmp(power, "sleeping"));
  assert(!net && usb && blocked && coverage_gap && !dirty && room_checked == 123);
  assert(!strcmp(last_worker_mode, "sync"));
  long long once = readonly_recheck_due_ms;
  test_ms += 5000; control_line(nudge);
  assert(readonly_recheck_due_ms == once && !worker);
  return 0;
}
