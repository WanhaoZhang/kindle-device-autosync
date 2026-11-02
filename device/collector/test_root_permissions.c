/* Exercise the exact fd-based production repair on temporary owned paths. */
#define main collector_program_main
#include "ksync-collector.c"
#undef main
#include <assert.h>
int main(int argc, char **argv) {
  assert(argc == 2);
  snprintf(root, sizeof root, "%s", argv[1]);
  char child[PATH_CAP], target[PATH_CAP];
  join(child, root, "keep.txt");
  int file = open(child, O_WRONLY | O_CREAT, 0644);
  assert(file >= 0);
  assert(fchmod(file, 0644) == 0);
  close(file);
  assert(chmod(root, 0755) == 0);
  assert(restrict_worker_root() == 0);
  struct stat s;
  assert(lstat(root, &s) == 0 && (s.st_mode & 0777) == 0700);
  assert(lstat(child, &s) == 0 && (s.st_mode & 0777) == 0644);
  assert(restrict_worker_root() == 0);
  /* Typed known files/FIFOs and bounded sealed logs are restored, unknown data
   * retains its original mode and contents. */
  join(child, root, "config.json");
  file = open(child, O_WRONLY | O_CREAT, 0660);
  assert(file >= 0 && write(file, "unchanged", 9) == 9);
  assert(fchmod(file, 0660) == 0); close(file);
  join(child, root, "control.fifo");
  assert(mkfifo(child, 0600) == 0 && chmod(child, 0660) == 0);
  char events[PATH_CAP], sealed[PATH_CAP], segment[PATH_CAP], unknown[PATH_CAP];
  join(events, root, "events"); assert(mkdir(events, 0770) == 0);
  join(sealed, events, "sealed"); assert(mkdir(sealed, 0770) == 0);
  join(segment, sealed, "segment-123-4-5.log");
  file = open(segment, O_CREAT | O_WRONLY, 0660); assert(file >= 0); close(file);
  assert(chmod(segment, 0660) == 0);
  join(unknown, sealed, "unknown.log");
  file = open(unknown, O_CREAT | O_WRONLY, 0660); assert(file >= 0); close(file);
  assert(chmod(unknown, 0660) == 0);
  assert(chmod(root, 02770) == 0);
  assert(restrict_worker_root() == 0);
  assert(lstat(root, &s) == 0 && (s.st_mode & 07777) == 0700);
  join(child, root, "config.json");
  assert(lstat(child, &s) == 0 && (s.st_mode & 07777) == 0600);
  file = open(child, O_RDONLY); char contents[10] = {0};
  assert(file >= 0 && read(file, contents, 9) == 9); close(file);
  assert(!strcmp(contents, "unchanged"));
  assert(lstat(events, &s) == 0 && (s.st_mode & 07777) == 0700);
  assert(lstat(sealed, &s) == 0 && (s.st_mode & 07777) == 0700);
  assert(lstat(segment, &s) == 0 && (s.st_mode & 07777) == 0600);
  assert(lstat(unknown, &s) == 0 && (s.st_mode & 07777) == 0660);
  join(child, root, "control.fifo");
  assert(lstat(child, &s) == 0 && S_ISFIFO(s.st_mode) && (s.st_mode & 07777) == 0600);
  /* A known file symlink or unexpected FIFO type must never be repaired. */
  join(child, root, "auth.json"); assert(symlink(unknown, child) == 0);
  assert(restrict_worker_root() == -1);
  assert(lstat(unknown, &s) == 0 && (s.st_mode & 07777) == 0660);
  assert(unlink(child) == 0);
  join(child, root, "control.fifo"); assert(unlink(child) == 0);
  file = open(child, O_CREAT | O_WRONLY, 0660); assert(file >= 0); close(file);
  assert(chmod(child, 0660) == 0 && restrict_worker_root() == -1);
  assert(lstat(child, &s) == 0 && (s.st_mode & 07777) == 0660);
  assert(unlink(child) == 0);
  assert(snprintf(target, sizeof target, "%s.target", root) > 0);
  assert(rename(root, target) == 0);
  assert(chmod(target, 0755) == 0);
  assert(symlink(target, root) == 0);
  assert(restrict_worker_root() == -1);
  assert(lstat(target, &s) == 0 && (s.st_mode & 0777) == 0755);
  assert(unlink(root) == 0);
  assert(restrict_worker_root() == -1);
  file = open(root, O_WRONLY | O_CREAT, 0644);
  assert(file >= 0);
  close(file);
  assert(restrict_worker_root() == -1);
  /* If the fixture runs as root, a directory owned by someone else is refused. */
  assert(unlink(root) == 0);
  assert(rename(target, root) == 0);
  if (geteuid() == 0) {
    assert(chown(root, 1, 1) == 0);
    assert(restrict_worker_root() == -1);
    assert(lstat(root, &s) == 0 && (s.st_mode & 0777) == 0755);
    assert(chown(root, 0, 0) == 0);
  }
  return 0;
}
