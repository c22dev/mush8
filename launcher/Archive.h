#ifndef MUSH8_ARCHIVE_H
#define MUSH8_ARCHIVE_H
#include <sys/types.h>
#include <stdint.h>

// macOS ships libarchive, but recent SDKs omit its public headers.
struct archive;
struct archive_entry;
struct archive *archive_read_new(void);
int archive_read_support_format_zip(struct archive *);
int archive_read_support_format_7zip(struct archive *);
int archive_read_support_format_rar(struct archive *);
int archive_read_support_format_rar5(struct archive *);
int archive_read_open_filename(struct archive *, const char *, size_t);
int archive_read_next_header(struct archive *, struct archive_entry **);
ssize_t archive_read_data(struct archive *, void *, size_t);
int archive_read_free(struct archive *);
const char *archive_error_string(struct archive *);
const char *archive_entry_pathname(struct archive_entry *);
mode_t archive_entry_filetype(struct archive_entry *);
int64_t archive_entry_size(struct archive_entry *);
const char *archive_entry_symlink(struct archive_entry *);
const char *archive_entry_hardlink(struct archive_entry *);
#endif
