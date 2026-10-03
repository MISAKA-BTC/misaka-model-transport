/*
 * misaka-transport libtorrent shim — a C ABI over libtorrent-rasterbar 2.0.x (RFC-0001 §6.1).
 *
 * No C++ type crosses this boundary. Every function catches every exception and returns a
 * status: 0 on success, a negative value on failure with a NUL-terminated message in `err`.
 * Handles are opaque 64-bit ids that the shim maps to torrent handles.
 *
 * What the shim never does: BEP 44 puts or gets, BEP 46, custom extension messages, an HTTP
 * listener, or anything with the content of a file but read and write it as pieces.
 */
#ifndef MISAKA_LT_SHIM_H
#define MISAKA_LT_SHIM_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct mt_session mt_session;

typedef struct mt_settings {
    uint16_t listen_port;
    int64_t upload_rate;   /* bytes/s, 0 = unlimited */
    int64_t download_rate; /* bytes/s, 0 = unlimited */
    int32_t connections;
    int32_t upnp_natpmp;   /* bool */
    int32_t lsd;           /* bool */
    int32_t dht;           /* bool */
    const char *dht_bootstrap; /* "host:port,host:port", may be empty */
    const char *state_dir;     /* the session state (DHT node id) is kept here */
    /* RFC-0002: an I2P mode when i2p_sam_host is non-empty. The swarm is then reached only through the
     * SAM bridge: no DHT, LSD, UPnP, NAT-PMP or clearnet peer, every torrent flagged i2p_torrent and
     * announced to `trackers` ("url,url", .i2p only). */
    const char *i2p_sam_host;
    uint16_t i2p_sam_port;
    int32_t i2p_inbound_length;
    int32_t i2p_outbound_length;
    int32_t i2p_inbound_quantity;
    int32_t i2p_outbound_quantity;
    const char *trackers;
} mt_settings;

enum {
    MT_STATE_METADATA = 0,
    MT_STATE_CHECKING = 1,
    MT_STATE_DOWNLOADING = 2,
    MT_STATE_FINISHED = 3,
    MT_STATE_SEEDING = 4,
    MT_STATE_PAUSED = 5,
    MT_STATE_ERROR = 6,
};

typedef struct mt_status_t {
    int32_t state;
    uint64_t total;
    uint64_t done;
    uint64_t uploaded;
    uint64_t downloaded;
    uint64_t upload_rate;
    uint64_t download_rate;
    uint32_t peers;
    uint32_t seeds;
    char error[256];
} mt_status_t;

enum {
    MT_EV_METADATA = 1,
    MT_EV_FINISHED = 2,
    MT_EV_HASH_FAILED = 3,
    MT_EV_PEER_BANNED = 4,
    MT_EV_ERROR = 5,
};

typedef struct mt_event {
    int32_t kind;
    uint64_t handle;
    uint64_t piece;
    char message[256];
} mt_event;

enum { MT_MODE_FETCH = 0, MT_MODE_SEED = 1, MT_MODE_ADOPT = 2 };

mt_session *mt_session_create(const mt_settings *s, char *err, size_t errlen);
void mt_session_destroy(mt_session *s);
int mt_apply_limits(mt_session *s, int64_t upload_rate, int64_t download_rate, int32_t connections, char *err, size_t errlen);

/* Adds a torrent by its v2 infohash with every file at priority 0: nothing is downloaded and no
 * file is created before mt_admit. */
int mt_add_magnet(mt_session *s, const uint8_t infohash[32], const char *save_path, uint64_t *handle_out, char *err,
                  size_t errlen);
/* The info dictionary of a torrent whose metadata arrived. *len_out is set to the full length;
 * returns -2 if it does not fit in cap. */
int mt_get_metadata(mt_session *s, uint64_t handle, uint8_t *buf, size_t cap, size_t *len_out, char *err, size_t errlen);
/* Sets every file to the default priority: the bundle passed the profile. */
int mt_admit(mt_session *s, uint64_t handle, char *err, size_t errlen);
/* Adds a bencoded .torrent ({info, piece layers}). Seed and adopt are seed mode plus upload mode:
 * the engine never writes into those files. */
int mt_add_torrent(mt_session *s, const uint8_t *torrent, size_t len, const char *save_path, int32_t mode,
                   uint64_t *handle_out, char *err, size_t errlen);
int mt_pause(mt_session *s, uint64_t handle, char *err, size_t errlen);
int mt_resume(mt_session *s, uint64_t handle, char *err, size_t errlen);
int mt_remove(mt_session *s, uint64_t handle, int32_t delete_files, char *err, size_t errlen);
int mt_status(mt_session *s, uint64_t handle, mt_status_t *out, char *err, size_t errlen);
/* The piece layers as a bencoded dictionary {pieces root: layer}. Same length protocol as
 * mt_get_metadata. */
int mt_piece_layers(mt_session *s, uint64_t handle, uint8_t *buf, size_t cap, size_t *len_out, char *err, size_t errlen);
size_t mt_pop_events(mt_session *s, mt_event *out, size_t max);

#ifdef __cplusplus
}
#endif

#endif
