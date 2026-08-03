/*
 * Embedded guacd shim - implementation.
 *
 * Reuses guacd's own connection.c / proc.c / proc-map.c verbatim (compiled
 * alongside this file). The only pieces we replace are guacd's daemon.c (the
 * accept() loop that binds a listening socket) and the conf-*.c CLI parsers.
 * Instead of accepting TCP connections, guac_embed_connect() manufactures a
 * connection over an AF_UNIX socketpair and runs guacd's stock
 * guacd_connection_thread() on one end.
 */

/* config.h must come first: guacamole's headers rely on the feature macros it
 * defines. This mirrors the include order used by guacd's own sources. */
#include "config.h"

#include "shim.h"

/* guacd internal headers (compiled from the vendored source tree). These pull
 * in the libguac headers (client.h, socket.h, parser.h, ...) in the correct
 * order, so connection.h is self-sufficient for guac_socket / guac_parser. */
#include "connection.h"
#include "log.h"
#include "proc-map.h"

#include <guacamole/client.h>
#include <guacamole/mem.h>

#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <syslog.h>
#include <sys/socket.h>
#include <unistd.h>

/*
 * Note: guacd_log_level is defined by the compiled-in log.c and declared
 * `extern` via log.h, so we only assign to it here (in guac_embed_init) rather
 * than defining it. daemon.c is deliberately excluded (that is where the
 * listening socket lives).
 */

struct guac_embed_ctx {

    /**
     * The shared map of all live connections, exactly as used by upstream
     * guacd. Persists for the life of the embedded instance.
     */
    guacd_proc_map* map;

};

guac_embed_ctx* guac_embed_init(int log_level) {

    /* Match daemon.c: set the max log level and open syslog. */
    guacd_log_level = log_level;
    openlog(GUACD_LOG_NAME, LOG_PID, LOG_DAEMON);

    guac_embed_ctx* ctx = malloc(sizeof(guac_embed_ctx));
    if (ctx == NULL)
        return NULL;

    ctx->map = guacd_proc_map_alloc();
    if (ctx->map == NULL) {
        free(ctx);
        return NULL;
    }

    return ctx;

}

int guac_embed_connect(guac_embed_ctx* ctx) {

    if (ctx == NULL)
        return -1;

    /* The pair of connected file descriptors standing in for what would
     * otherwise be an accept()ed TCP connection. fds[0] is guacd's end,
     * fds[1] is handed back to the caller. */
    int fds[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, fds) != 0)
        return -1;

    /* Allocated with guac_mem_alloc() because guacd_connection_thread() frees
     * this with guac_mem_free(). */
    guacd_connection_thread_params* params =
        guac_mem_alloc(sizeof(guacd_connection_thread_params));

    if (params == NULL) {
        close(fds[0]);
        close(fds[1]);
        return -1;
    }

    memset(params, 0, sizeof(guacd_connection_thread_params));
    params->map = ctx->map;
    params->connected_socket_fd = fds[0];
#ifdef ENABLE_SSL
    /* No TLS termination for in-process connections. */
    params->ssl_context = NULL;
#endif

    pthread_t thread;
    if (pthread_create(&thread, NULL, guacd_connection_thread, params) != 0) {
        guac_mem_free(params);
        close(fds[0]);
        close(fds[1]);
        return -1;
    }

    /* Fire-and-forget: guacd's connection thread lives for the duration of the
     * connection and cleans up fds[0] and the forked client process itself. */
    pthread_detach(thread);

    return fds[1];

}

void guac_embed_shutdown(guac_embed_ctx* ctx) {

    if (ctx == NULL)
        return;

    guacd_proc_map_free(ctx->map);
    free(ctx);

}
