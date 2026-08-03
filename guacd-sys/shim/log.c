/*
 * Embedded guacd logging - routes guacd's log output to the host Rust process
 * instead of syslog/stderr.
 *
 * This is a drop-in replacement for guacamole-server's src/guacd/log.c (which
 * build.rs deliberately excludes from the compile). It keeps every symbol that
 * guacd's connection.c / proc.c reference -- guacd_log_level, vguacd_log,
 * guacd_log, guacd_client_log, guacd_log_guac_error, guacd_log_handshake_failure
 * -- but instead of syslog + stderr it frames each formatted message and writes
 * it down a pipe created by guac_embed_init(). A reader thread on the Rust side
 * drains the pipe and re-emits every record through the `log` crate.
 *
 * Why a pipe and not a direct FFI callback: guacd runs each protocol client in
 * a forked (no-exec) child process. Calling back into Rust from such a child is
 * unsafe in a multithreaded host -- the child inherits a frozen snapshot of
 * every lock, so a logger mutex held by another thread at fork() time would
 * deadlock the child forever. write() is async-signal-safe and, for records
 * under PIPE_BUF bytes, atomic against interleaving from other writers, so a
 * forked client can log safely without touching Rust or taking any lock.
 */

#include "config.h"
#include "log.h"

#include <guacamole/client.h>
#include <guacamole/error.h>

#include <stdarg.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int guacd_log_level = GUAC_LOG_INFO;

/**
 * Largest message payload vguacd_log will format. Matches upstream guacd's
 * buffer size. The framed record (see below) stays well under PIPE_BUF, so a
 * single write() is atomic even when several forked clients log at once.
 */
#define GUAC_EMBED_LOG_MSG_MAX 2048

/*
 * Write end of the log pipe, inherited by every forked client process. Remains
 * -1 until guac_embed_log_set_fd() runs (in guac_embed_init), in which case
 * vguacd_log falls back to stderr so early/unconfigured logging is not lost.
 */
static int guac_embed_log_fd = -1;

/* Wired up from shim.c; declared there via matching extern prototypes. */
void guac_embed_log_set_fd(int fd) {
    guac_embed_log_fd = fd;
}

void guac_embed_log_close_fd(void) {
    if (guac_embed_log_fd >= 0) {
        close(guac_embed_log_fd);
        guac_embed_log_fd = -1;
    }
}

void vguacd_log(guac_client_log_level level, const char* format,
        va_list args) {

    char message[GUAC_EMBED_LOG_MSG_MAX];

    /* Don't bother if the log level is too high */
    if ((int) level > guacd_log_level)
        return;

    /* Copy log message into buffer. vsnprintf returns the length it *would*
     * have written; clamp to what actually fits. */
    int written = vsnprintf(message, sizeof(message), format, args);
    if (written < 0)
        return;

    size_t msg_len = (size_t) written;
    if (msg_len >= sizeof(message))
        msg_len = sizeof(message) - 1;

    /* Fall back to stderr if the host never wired up the pipe. */
    if (guac_embed_log_fd < 0) {
        fprintf(stderr, GUACD_LOG_NAME "[%i]: %.*s\n",
                (int) getpid(), (int) msg_len, message);
        return;
    }

    /* Frame: [level:1][length:2, little-endian][message:length]. The whole
     * record is a single write() and stays under PIPE_BUF, so it reaches the
     * reader intact even when several forked clients log concurrently. */
    unsigned char record[3 + GUAC_EMBED_LOG_MSG_MAX];
    record[0] = (unsigned char) level;
    record[1] = (unsigned char) (msg_len & 0xFF);
    record[2] = (unsigned char) ((msg_len >> 8) & 0xFF);
    memcpy(record + 3, message, msg_len);

    /* Best-effort: a failed or short write drops the line rather than blocking
     * the connection. */
    ssize_t rc = write(guac_embed_log_fd, record, 3 + msg_len);
    (void) rc;

}

void guacd_log(guac_client_log_level level, const char* format, ...) {
    va_list args;
    va_start(args, format);
    vguacd_log(level, format, args);
    va_end(args);
}

void guacd_client_log(guac_client* client, guac_client_log_level level,
        const char* format, va_list args) {
    vguacd_log(level, format, args);
}

void guacd_log_guac_error(guac_client_log_level level, const char* message) {

    if (guac_error != GUAC_STATUS_SUCCESS) {

        /* If error message provided, include in log */
        if (guac_error_message != NULL)
            guacd_log(level, "%s: %s",
                    message,
                    guac_error_message);

        /* Otherwise just log with standard status string */
        else
            guacd_log(level, "%s: %s",
                    message,
                    guac_status_string(guac_error));

    }

    /* Just log message if no status code */
    else
        guacd_log(level, "%s", message);

}

void guacd_log_handshake_failure() {

    if (guac_error == GUAC_STATUS_CLOSED)
        guacd_log(GUAC_LOG_DEBUG,
                "Guacamole connection closed during handshake");
    else if (guac_error == GUAC_STATUS_PROTOCOL_ERROR)
        guacd_log(GUAC_LOG_ERROR,
                "Guacamole protocol violation. Perhaps the version of "
                "guacamole-client is incompatible with this version of "
                "guacd?");
    else
        guacd_log(GUAC_LOG_WARNING,
                "Guacamole handshake failed: %s",
                guac_status_string(guac_error));

}
