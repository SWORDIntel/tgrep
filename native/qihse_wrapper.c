/**
 * qihse_wrapper.c — tgrep narrow FFI wrapper around QIHSE file helpers.
 *
 * Currently just fsync/dir-sync helpers for crash-safe segment publication.
 * Will grow as QIHSE WAL, index manager, and FTS are wired in later phases.
 */

#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <stdio.h>

/* fsync a file descriptor. Returns 0 on success, -1 on error. */
int tgrep_qihse_file_sync(int fd) {
    return fsync(fd);
}

/* Open a directory and fsync it (ensures directory entries are durable).
 * Returns 0 on success, -1 on error. */
int tgrep_qihse_dir_sync(const char* path) {
    if (!path) return -1;
    int fd = open(path, O_RDONLY | O_DIRECTORY);
    if (fd < 0) return -1;
    int rc = fsync(fd);
    close(fd);
    return rc;
}

/* Atomic file write: write data to tmp file, fsync, rename to target, fsync dir.
 * Returns 0 on success, -1 on error.
 * This is the crash-safe segment publication primitive. */
int tgrep_qihse_atomic_write(
    const char* target_path,
    const uint8_t* data,
    size_t len
) {
    if (!target_path || (!data && len > 0)) return -1;

    /* Build tmp path: target_path + ".tmp" */
    size_t path_len = strlen(target_path);
    char tmp_path[4096];
    if (path_len + 5 >= sizeof(tmp_path)) return -1;
    memcpy(tmp_path, target_path, path_len);
    memcpy(tmp_path + path_len, ".tmp", 5);

    /* Write to tmp file */
    int fd = open(tmp_path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return -1;

    size_t written = 0;
    while (written < len) {
        ssize_t n = write(fd, data + written, len - written);
        if (n < 0) {
            close(fd);
            unlink(tmp_path);
            return -1;
        }
        written += (size_t)n;
    }

    /* fsync the tmp file */
    if (fsync(fd) < 0) {
        close(fd);
        unlink(tmp_path);
        return -1;
    }
    close(fd);

    /* Atomic rename */
    if (rename(tmp_path, target_path) < 0) {
        unlink(tmp_path);
        return -1;
    }

    /* fsync the parent directory */
    /* Extract directory from target_path */
    char dir_path[4096];
    size_t dir_len = path_len;
    while (dir_len > 0 && target_path[dir_len - 1] != '/') dir_len--;
    if (dir_len == 0) {
        memcpy(dir_path, ".", 2);
    } else {
        memcpy(dir_path, target_path, dir_len);
        dir_path[dir_len] = '\0';
    }

    return tgrep_qihse_dir_sync(dir_path);
}
