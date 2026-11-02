/* SPDX-License-Identifier: AGPL-3.0-only
 * Repair only typed, owned project entries. No contents, recursion or chown. */
#ifndef KSYNC_PRIVATE_PERMISSIONS_H
#define KSYNC_PRIVATE_PERMISSIONS_H
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
enum ksync_private_kind { KSYNC_DIRECTORY, KSYNC_REGULAR, KSYNC_FIFO };
static inline int ksync_private_fd(int fd, enum ksync_private_kind kind,
                                   mode_t mode, uid_t owner) {
  struct stat s;
  if (fstat(fd, &s) || s.st_uid != owner ||
      (kind == KSYNC_DIRECTORY ? !S_ISDIR(s.st_mode) :
       kind == KSYNC_REGULAR ? !S_ISREG(s.st_mode) : !S_ISFIFO(s.st_mode)) ||
      (kind != KSYNC_DIRECTORY && s.st_nlink != 1)) {
    errno = EPERM;
    return -1;
  }
  if ((s.st_mode & 07777) != mode && fchmod(fd, mode))
    return -1;
  if (fstat(fd, &s) || s.st_uid != owner || (s.st_mode & 07777) != mode) {
    errno = EPERM;
    return -1;
  }
  return 0;
}
/* -2 means a missing optional entry. All existing entries must pass exactly. */
static inline int ksync_private_openat(int parent, const char *name,
                                       enum ksync_private_kind kind,
                                       mode_t mode, uid_t owner) {
  int flags = O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC;
  if (kind == KSYNC_DIRECTORY)
    flags |= O_DIRECTORY;
  int fd = openat(parent, name, flags);
  if (fd < 0)
    return errno == ENOENT ? -2 : -1;
  if (ksync_private_fd(fd, kind, mode, owner)) {
    int saved = errno;
    close(fd);
    errno = saved;
    return -1;
  }
  return fd;
}
static inline int ksync_private_optional(int parent, const char *name,
                                        enum ksync_private_kind kind,
                                        mode_t mode, uid_t owner) {
  int fd = ksync_private_openat(parent, name, kind, mode, owner);
  if (fd == -2)
    return 0;
  if (fd < 0)
    return -1;
  return close(fd);
}
static inline int ksync_private_root(const char *root, uid_t owner) {
  int fd = open(root, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
  if (fd < 0)
    return -1;
  if (ksync_private_fd(fd, KSYNC_DIRECTORY, 0700, owner)) {
    int saved = errno;
    close(fd);
    errno = saved;
    return -1;
  }
  return fd;
}
static inline int ksync_private_channels_at(int fd, uid_t owner) {
  return ksync_private_optional(fd, "control.fifo", KSYNC_FIFO, 0600, owner) ||
         ksync_private_optional(fd, "adapter-control.fifo", KSYNC_FIFO, 0600, owner)
             ? -1 : 0;
}
static inline int ksync_restrict_control_channels(const char *root, uid_t owner) {
  int fd = ksync_private_root(root, owner);
  if (fd < 0)
    return -1;
  int result = ksync_private_channels_at(fd, owner);
  close(fd);
  return result;
}
static inline int ksync_private_segment_name(const char *name) {
  size_t n = strlen(name);
  if (n < 13 || strncmp(name, "segment-", 8) || strcmp(name + n - 4, ".log"))
    return 0;
  for (size_t i = 8; i < n - 4; i++)
    if ((name[i] < '0' || name[i] > '9') && name[i] != '-')
      return 0;
  return 1;
}
static inline int ksync_restrict_private_state(const char *root, uid_t owner) {
  static const char *files[] = {
    "config.json", "auth.json", "runtime.json", "validation-tls.json",
    "experimental-authorization.json", "validation-device.json",
    "collection-state.json", "sidecar-index.json", "catalog-index.json",
    "worker.lock", "collector.lock", "collector.checkpoint", "ledger.sqlite",
    "ledger.sqlite-journal", "ledger.sqlite-wal", "ledger.sqlite-shm", "disabled",
    "readonly-recheck.pending"
  };
  int fd = ksync_private_root(root, owner);
  if (fd < 0)
    return -1;
  int result = -1, events = -1, sealed = -1, ota = -1;
  for (size_t i = 0; i < sizeof files / sizeof files[0]; i++)
    if (ksync_private_optional(fd, files[i], KSYNC_REGULAR, 0600, owner))
      goto done;
  if (ksync_private_channels_at(fd, owner) ||
      ksync_private_optional(fd, "bin", KSYNC_DIRECTORY, 0700, owner) ||
      ksync_private_optional(fd, "install", KSYNC_DIRECTORY, 0700, owner))
    goto done;
  ota = ksync_private_openat(fd, "ota", KSYNC_DIRECTORY, 0700, owner);
  if (ota == -1 || (ota >= 0 &&
      ksync_private_optional(ota, "state.tsv", KSYNC_REGULAR, 0600, owner)))
    goto done;
  events = ksync_private_openat(fd, "events", KSYNC_DIRECTORY, 0700, owner);
  if (events == -1)
    goto done;
  if (events >= 0) {
    if (ksync_private_optional(events, "active.log", KSYNC_REGULAR, 0600, owner))
      goto done;
    sealed = ksync_private_openat(events, "sealed", KSYNC_DIRECTORY, 0700, owner);
    if (sealed == -1)
      goto done;
    if (sealed >= 0) {
      DIR *dir = fdopendir(dup(sealed));
      if (!dir)
        goto done;
      unsigned entries = 0;
      struct dirent *entry;
      int bad = 0;
      errno = 0;
      while ((entry = readdir(dir))) {
        if (!strcmp(entry->d_name, ".") || !strcmp(entry->d_name, ".."))
          continue;
        if (++entries > 512 || (ksync_private_segment_name(entry->d_name) &&
            ksync_private_optional(sealed, entry->d_name, KSYNC_REGULAR, 0600, owner))) {
          bad = 1;
          break;
        }
        errno = 0;
      }
      if (errno)
        bad = 1;
      closedir(dir);
      if (bad)
        goto done;
    }
  }
  result = 0;
done:
  if (ota >= 0) close(ota);
  if (sealed >= 0) close(sealed);
  if (events >= 0) close(events);
  close(fd);
  if (result && !errno) errno = EPERM;
  return result;
}
#endif
