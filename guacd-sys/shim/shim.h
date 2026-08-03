/*
 * Embedded guacd shim - public C API.
 *
 * This is the ONLY surface bindgen is pointed at. It deliberately exposes no
 * guacamole/libguac types so that the generated Rust bindings stay tiny.
 */

#ifndef GUAC_EMBED_SHIM_H
#define GUAC_EMBED_SHIM_H

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Opaque handle to an embedded guacd instance. Holds the shared process map
 * that guacd uses to track live connections. Allocated by guac_embed_init(),
 * released by guac_embed_shutdown().
 */
typedef struct guac_embed_ctx guac_embed_ctx;

/**
 * Initializes guacd's in-process machinery WITHOUT binding or listening on any
 * socket. This is the embedded analogue of starting the guacd daemon.
 *
 * @param log_level
 *     The maximum guac_client_log_level to emit (3=ERROR, 4=WARNING, 6=INFO,
 *     7=DEBUG, 8=TRACE), matching guacamole's guac_client_log_level enum.
 *
 * @return
 *     A newly-allocated context, or NULL on allocation failure.
 */
guac_embed_ctx* guac_embed_init(int log_level);

/**
 * Returns the read end of the log pipe for the given context. guacd's log
 * output -- including messages emitted by forked protocol-client processes --
 * is framed and written to the matching write end. Each record is:
 *
 *     [level: 1 byte][length: 2 bytes, little-endian][message: length bytes]
 *
 * where `level` is a guac_client_log_level (3=ERROR, 4=WARNING, 6=INFO,
 * 7=DEBUG, 8=TRACE) and `message` is UTF-8 text with no trailing newline. The
 * caller is expected to read records from this fd (e.g. on a dedicated thread)
 * and forward them to its own logging facility, and takes ownership of the fd.
 *
 * @param ctx
 *     The context returned by guac_embed_init().
 *
 * @return
 *     The read-end file descriptor, or -1 if the pipe could not be created (in
 *     which case guacd logs to stderr instead).
 */
int guac_embed_log_fd(guac_embed_ctx* ctx);

/**
 * Creates a new in-process Guacamole connection. Internally this allocates an
 * AF_UNIX socketpair, hands one end to a detached guacd connection thread
 * (which reads the "select" instruction, forks the protocol-specific client
 * process, and loads its plugin exactly as guacd does for a TCP client), and
 * returns the other end to the caller.
 *
 * The caller speaks the Guacamole protocol over the returned file descriptor,
 * exactly as the web tier would over a TCP connection to guacd.
 *
 * @param ctx
 *     The context returned by guac_embed_init().
 *
 * @return
 *     A file descriptor for the caller's end of the connection on success, or
 *     -1 on error (errno is set).
 */
int guac_embed_connect(guac_embed_ctx* ctx);

/**
 * Frees the given context and its process map. Does not forcibly terminate
 * in-flight connection threads; closing the caller-side file descriptors is
 * what ends each connection and reaps its forked client process.
 *
 * @param ctx
 *     The context to free. May be NULL.
 */
void guac_embed_shutdown(guac_embed_ctx* ctx);

#ifdef __cplusplus
}
#endif

#endif
