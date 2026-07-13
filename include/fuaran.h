/*
 * fuaran.h — the C-ABI surface of fuaran-rs (the Rust host of the Fuaran UI wire
 * format). Hand-written (Phase 537); the stdlib-only mandate forbids a cbindgen
 * build dependency, though cbindgen MAY run as a dev-tooling *check* that this
 * header still matches the exported `#[no_mangle]` surface in `src/ffi/`.
 *
 * This header declares the target-neutral session surface a native binding
 * (the Swift XCFramework, the Kotlin cargo-ndk `.so`) links against. The same
 * symbols are emitted into the `wasm32` browser module, where the thin JS loader
 * (`js/fuaran-loader.js`) drives them — see the RETURN ABI note below for the one
 * representational difference between the two worlds.
 *
 * SPDX-License-Identifier: Apache-2.0
 * Copyright Diametrical Ltd.
 */

#ifndef FUARAN_H
#define FUARAN_H

#include <stddef.h> /* size_t */
#include <stdint.h> /* uint8_t, uint64_t */

#ifdef __cplusplus
extern "C" {
#endif

/* ------------------------------------------------------------------------- *
 * Opaque session handle
 *
 * A live client session over one decoded wire tree: it holds the current tree
 * plus its binding sources, renders to HTML, applies tree-ops, and writes the
 * reactive stores. Created by fuaran_session_new, destroyed by
 * fuaran_session_free. The layout is private to Rust — only ever hold a pointer.
 * ------------------------------------------------------------------------- */
typedef struct FuaranSession FuaranSession;

/* ------------------------------------------------------------------------- *
 * Output buffer — the (ptr, len) RETURN ABI
 *
 * Every function that returns text to the caller returns a Rust-OWNED buffer as
 * a (ptr, len) pair. The caller reads `len` UTF-8 bytes at `ptr`, then MUST free
 * the buffer with fuaran_dealloc(ptr, len). The buffer is exact-length: there is
 * NO trailing NUL — always honour `len`, never strlen().
 *
 * The pair's REPRESENTATION is pointer-width dependent, because a full pointer
 * only shares a 64-bit word with a length losslessly when pointers are 32-bit:
 *
 *   - Native targets (64-bit — this header's audience): the pair is this
 *     two-word struct, returned by value.
 *   - wasm32 (32-bit pointers): the same pair is a packed uint64_t — `ptr` in
 *     the high 32 bits, `len` in the low 32 — so the JS loader stays byte-for-
 *     byte unchanged. Native code never sees the packed form; it is documented
 *     here only so the two ABIs are on the record together.
 * ------------------------------------------------------------------------- */
typedef struct FuaranBuf {
  uint8_t *ptr; /* Rust-owned; free with fuaran_dealloc(ptr, len). */
  size_t len;   /* exact byte length; NOT NUL-terminated. */
} FuaranBuf;

/* ------------------------------------------------------------------------- *
 * Buffer lifecycle
 * ------------------------------------------------------------------------- */

/*
 * Allocate a zeroed INPUT buffer of `len` bytes. The caller writes UTF-8 into it,
 * passes (ptr, len) to a consuming call, then frees it with fuaran_dealloc AFTER
 * that call returns. Rust only borrows an input buffer for the call's duration.
 * Returns NULL only if the allocation of `len` bytes fails.
 */
uint8_t *fuaran_alloc(size_t len);

/*
 * Free a buffer — either an INPUT buffer from fuaran_alloc, or the OUTPUT buffer
 * inside a returned FuaranBuf. (ptr, len) must be a pair this library produced,
 * freed EXACTLY ONCE. A NULL `ptr` is a no-op.
 */
void fuaran_dealloc(uint8_t *ptr, size_t len);

/* ------------------------------------------------------------------------- *
 * Session lifecycle
 * ------------------------------------------------------------------------- */

/*
 * Decode a canonical wire `Node` JSON (UTF-8 at ptr/len) into a new session.
 * Returns an owned handle, or NULL on a decode failure — in which case
 * fuaran_last_error() returns the failure's JSON envelope (read it on THIS
 * thread; see the threading contract). Free a non-NULL handle with
 * fuaran_session_free.
 */
FuaranSession *fuaran_session_new(const uint8_t *ptr, size_t len);

/* Free a session handle from fuaran_session_new. NULL is a no-op. */
void fuaran_session_free(FuaranSession *session);

/* ------------------------------------------------------------------------- *
 * Session operations — all return an owned FuaranBuf (free with fuaran_dealloc)
 * ------------------------------------------------------------------------- */

/*
 * Render the session's current tree to a body-fragment HTML string. Byte-
 * identical to the server renderer for the same tree + sources (the hydration-
 * parity contract). A NULL session returns an empty buffer.
 */
FuaranBuf fuaran_session_render(FuaranSession *session);

/*
 * The session's current tree, re-encoded to canonical wire JSON — the round-trip
 * exit point (persist / diff / hand back to a headless peer). NULL → empty.
 */
FuaranBuf fuaran_session_tree_json(FuaranSession *session);

/*
 * Apply a canonical wire `TreeOp` JSON (UTF-8 at ptr/len). Returns {"ok":true}
 * on success (the session adopts the new tree) or a structured error envelope
 * on failure (the held tree is UNTOUCHED — the apply engine never partially
 * mutates):
 *   {"error":{"class":"decode"|"apply","code":"...","message":"..."[,"path":"..."][,"batchIndex":N]}}
 * NULL session → empty buffer.
 */
FuaranBuf fuaran_session_apply_op(FuaranSession *session, const uint8_t *ptr,
                                  size_t len);

/*
 * Write a reactive `$state.<key>` slot from a JSON value (the write-back an
 * omitted-handler control performs). Re-render to observe the change. Returns
 * {"ok":true} or a decode-error envelope. key/val are UTF-8 (ptr, len) pairs.
 */
FuaranBuf fuaran_session_set_state(FuaranSession *session, const uint8_t *key_ptr,
                                   size_t key_len, const uint8_t *val_ptr,
                                   size_t val_len);

/* Write a `$filters.<name>` slot. Same shape as fuaran_session_set_state. */
FuaranBuf fuaran_session_set_filter(FuaranSession *session, const uint8_t *key_ptr,
                                    size_t key_len, const uint8_t *val_ptr,
                                    size_t val_len);

/*
 * Seed a `$queries.<name>` result slot from a JSON value (host-fed data for a
 * `Binding.Query` consumer — the fetch is a host concern; the tree owns only the
 * name). Same shape as fuaran_session_set_state.
 */
FuaranBuf fuaran_session_set_query(FuaranSession *session, const uint8_t *key_ptr,
                                   size_t key_len, const uint8_t *val_ptr,
                                   size_t val_len);

/*
 * The last fuaran_session_new failure envelope (owned FuaranBuf), or an empty
 * buffer when the last `new` on this thread succeeded. See the threading
 * contract: the slot is per-thread.
 */
FuaranBuf fuaran_last_error(void);

/* ------------------------------------------------------------------------- *
 * OWNERSHIP CONTRACT (summary)
 *
 *   - INPUT buffers  (fuaran_alloc):  caller-owned. Rust borrows for one call;
 *                                     caller frees with fuaran_dealloc after.
 *   - OUTPUT buffers (inside FuaranBuf): Rust-owned. Caller reads `len` bytes,
 *                                     then frees with fuaran_dealloc(ptr, len).
 *   - SESSION handle (fuaran_session_new): caller-owned; free ONCE with
 *                                     fuaran_session_free.
 *   Every buffer is freed exactly once through fuaran_dealloc; a session exactly
 *   once through fuaran_session_free. Output buffers carry NO trailing NUL.
 *
 * THREADING CONTRACT
 *
 *   A FuaranSession is SINGLE-OWNER: confine it (and all calls that take it) to
 *   ONE thread / executor for its whole lifetime. No Send/Sync guarantee crosses
 *   this boundary — a Swift `actor` wrapper or a Kotlin single-threaded
 *   dispatcher is the intended confinement. fuaran_last_error() reads a
 *   PER-THREAD slot, so read it on the same thread that made the failing
 *   fuaran_session_new call. (On the single-threaded wasm32 host the per-thread
 *   slot degenerates to a single global.)
 *
 * v0 NATIVE SURFACE (Phase 537 decision)
 *
 *   v0 is exactly the session surface declared above. The adopted native-surface
 *   architecture keeps the Rust core as the owner of truth and mutation while a
 *   native tier holds a render-only projection, so ALL native rendering is driven
 *   through a session (session_apply_op -> read back session_tree_json). No
 *   additional entry points are needed for the native binding tiers.
 *
 *   RESERVED (not in v0; names held for a future stateless surface if demand
 *   appears — do not bind yet):
 *       FuaranBuf fuaran_validate(const uint8_t *ptr, size_t len);
 *       FuaranBuf fuaran_encode_canonical(const uint8_t *ptr, size_t len);
 *       FuaranBuf fuaran_apply(const uint8_t *tree_ptr, size_t tree_len,
 *                              const uint8_t *op_ptr, size_t op_len);
 * ------------------------------------------------------------------------- */

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* FUARAN_H */
