// misaka-transport libtorrent shim: see shim.h.
//
// Built only with the `libtorrent` feature of misaka-transport-engine, against the pinned
// libtorrent-rasterbar 2.0.x of third_party.toml.

#include "shim.h"

#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <fstream>
#include <iterator>
#include <map>
#include <memory>
#include <mutex>
#include <set>
#include <string>
#include <vector>

#include <libtorrent/add_torrent_params.hpp>
#include <libtorrent/alert_types.hpp>
#include <libtorrent/bencode.hpp>
#include <libtorrent/entry.hpp>
#include <libtorrent/error_code.hpp>
#include <libtorrent/info_hash.hpp>
#include <libtorrent/session.hpp>
#include <libtorrent/session_params.hpp>
#include <libtorrent/settings_pack.hpp>
#include <libtorrent/torrent_flags.hpp>
#include <libtorrent/torrent_handle.hpp>
#include <libtorrent/torrent_info.hpp>
#include <libtorrent/torrent_status.hpp>

namespace lt = libtorrent;

struct mt_session {
    std::unique_ptr<lt::session> ses;
    bool i2p = false;
    std::vector<std::string> trackers;
    std::string state_path;
    std::mutex mu;
    uint64_t next = 0;
    std::map<uint64_t, lt::torrent_handle> handles;
    std::map<lt::sha256_hash, uint64_t> by_hash;
    std::deque<mt_event> pending;
    // Handles whose files may be downloaded: admitted magnets and torrents added whole. A magnet
    // before admission wants nothing, and libtorrent calls that "finished".
    std::set<uint64_t> admitted;
};

namespace {

void set_err(char *err, size_t len, const std::string &msg) {
    if (err == nullptr || len == 0) return;
    size_t n = std::min(len - 1, msg.size());
    std::memcpy(err, msg.data(), n);
    err[n] = '\0';
}

void copy_msg(char (&dst)[256], const std::string &msg) {
    size_t n = std::min(sizeof(dst) - 1, msg.size());
    std::memcpy(dst, msg.data(), n);
    dst[n] = '\0';
}

lt::settings_pack make_pack(const mt_settings *s) {
    lt::settings_pack p;
    std::string port = std::to_string(s->listen_port);
    // TCP and uTP over IPv4 and IPv6, one port (§6.3).
    p.set_str(lt::settings_pack::listen_interfaces, "0.0.0.0:" + port + ",[::]:" + port);
    p.set_bool(lt::settings_pack::enable_dht, s->dht != 0);
    p.set_bool(lt::settings_pack::enable_lsd, s->lsd != 0);
    p.set_bool(lt::settings_pack::enable_upnp, s->upnp_natpmp != 0);
    p.set_bool(lt::settings_pack::enable_natpmp, s->upnp_natpmp != 0);
    if (s->dht_bootstrap != nullptr) p.set_str(lt::settings_pack::dht_bootstrap_nodes, s->dht_bootstrap);
    p.set_int(lt::settings_pack::upload_rate_limit, static_cast<int>(std::min<int64_t>(s->upload_rate, INT32_MAX)));
    p.set_int(lt::settings_pack::download_rate_limit, static_cast<int>(std::min<int64_t>(s->download_rate, INT32_MAX)));
    p.set_int(lt::settings_pack::connections_limit, s->connections);
    if (s->i2p_sam_host != nullptr && s->i2p_sam_host[0] != '\0') {
        // RFC-0002 §2: the swarm only through SAM. Nothing listens beyond loopback and no clearnet
        // discovery runs; the packet-level confinement of §5 is the guarantee, this is the intent.
        p.set_str(lt::settings_pack::listen_interfaces, "127.0.0.1:" + port);
        p.set_bool(lt::settings_pack::enable_dht, false);
        p.set_bool(lt::settings_pack::enable_lsd, false);
        p.set_bool(lt::settings_pack::enable_upnp, false);
        p.set_bool(lt::settings_pack::enable_natpmp, false);
        p.set_bool(lt::settings_pack::enable_incoming_utp, false);
        p.set_bool(lt::settings_pack::enable_outgoing_utp, false);
        p.set_bool(lt::settings_pack::allow_i2p_mixed, false);
        p.set_str(lt::settings_pack::i2p_hostname, s->i2p_sam_host);
        p.set_int(lt::settings_pack::i2p_port, s->i2p_sam_port);
        p.set_int(lt::settings_pack::i2p_inbound_length, s->i2p_inbound_length);
        p.set_int(lt::settings_pack::i2p_outbound_length, s->i2p_outbound_length);
        p.set_int(lt::settings_pack::i2p_inbound_quantity, s->i2p_inbound_quantity);
        p.set_int(lt::settings_pack::i2p_outbound_quantity, s->i2p_outbound_quantity);
        // Announce to every tracker: several operators, none of them trusted (RFC-0002 §4.1).
        p.set_bool(lt::settings_pack::announce_to_all_trackers, true);
        p.set_bool(lt::settings_pack::announce_to_all_tiers, true);
    }
    // Obfuscation against throttling, not confidentiality: the content is public (§6.3).
    p.set_int(lt::settings_pack::out_enc_policy, lt::settings_pack::pe_enabled);
    p.set_int(lt::settings_pack::in_enc_policy, lt::settings_pack::pe_enabled);
    // No MISAKA identity anywhere (MT-11): libtorrent's own fingerprint and user agent.
    p.set_int(lt::settings_pack::alert_mask, lt::alert_category::error | lt::alert_category::status |
                                                lt::alert_category::storage | lt::alert_category::peer |
                                                lt::alert_category::tracker |
                                                (std::getenv("MISAKA_LT_DEBUG") ? lt::alert_category::torrent_log |
                                                                                      lt::alert_category::session_log
                                                                                : lt::alert_category_t{}));
    return p;
}

lt::torrent_handle find(mt_session *s, uint64_t h) {
    auto it = s->handles.find(h);
    return it == s->handles.end() ? lt::torrent_handle() : it->second;
}

uint64_t track(mt_session *s, const lt::torrent_handle &th) {
    uint64_t id = ++s->next;
    s->handles[id] = th;
    s->by_hash[th.info_hashes().v2] = id;
    return id;
}

int copy_out(const std::string &bytes, uint8_t *buf, size_t cap, size_t *len_out) {
    *len_out = bytes.size();
    if (bytes.size() > cap) return -2;
    if (!bytes.empty()) std::memcpy(buf, bytes.data(), bytes.size());
    return 0;
}

#define MT_TRY try {
#define MT_CATCH                                                                                                       \
    }                                                                                                                  \
    catch (const std::exception &e) {                                                                                  \
        set_err(err, errlen, e.what());                                                                                \
        return -1;                                                                                                     \
    }                                                                                                                  \
    catch (...) {                                                                                                      \
        set_err(err, errlen, "unknown exception");                                                                     \
        return -1;                                                                                                     \
    }

}  // namespace

extern "C" {

mt_session *mt_session_create(const mt_settings *s, char *err, size_t errlen) {
    try {
        auto out = std::make_unique<mt_session>();
        lt::session_params params;
        if (s->state_dir != nullptr) {
            out->state_path = std::string(s->state_dir) + "/session.dat";
            std::ifstream in(out->state_path, std::ios::binary);
            if (in) {
                std::vector<char> buf((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
                // Only the DHT state is restored; settings are ours, not the file's.
                params = lt::read_session_params(buf, lt::session_handle::save_dht_state);
            }
        }
        params.settings = make_pack(s);
        out->i2p = s->i2p_sam_host != nullptr && s->i2p_sam_host[0] != '\0';
        if (s->trackers != nullptr) {
            std::string all(s->trackers);
            size_t start = 0;
            while (start <= all.size()) {
                size_t end = all.find(',', start);
                if (end == std::string::npos) end = all.size();
                if (end > start) out->trackers.push_back(all.substr(start, end - start));
                start = end + 1;
            }
        }
        if (out->i2p) params.dht_state = {};
        out->ses = std::make_unique<lt::session>(std::move(params));
        return out.release();
    } catch (const std::exception &e) {
        set_err(err, errlen, e.what());
    } catch (...) {
        set_err(err, errlen, "unknown exception");
    }
    return nullptr;
}

void mt_session_destroy(mt_session *s) {
    if (s == nullptr) return;
    try {
        if (!s->state_path.empty()) {
            std::vector<char> buf = lt::write_session_params_buf(s->ses->session_state(lt::session_handle::save_dht_state),
                                                                 lt::session_handle::save_dht_state);
            std::string tmp = s->state_path + ".tmp";
            std::ofstream out(tmp, std::ios::binary | std::ios::trunc);
            out.write(buf.data(), static_cast<std::streamsize>(buf.size()));
            out.close();
            std::rename(tmp.c_str(), s->state_path.c_str());
        }
    } catch (...) {
    }
    delete s;
}

int mt_apply_limits(mt_session *s, int64_t up, int64_t down, int32_t conns, char *err, size_t errlen) {
    MT_TRY
    std::lock_guard<std::mutex> g(s->mu);
    lt::settings_pack p;
    p.set_int(lt::settings_pack::upload_rate_limit, static_cast<int>(std::min<int64_t>(up, INT32_MAX)));
    p.set_int(lt::settings_pack::download_rate_limit, static_cast<int>(std::min<int64_t>(down, INT32_MAX)));
    p.set_int(lt::settings_pack::connections_limit, conns);
    s->ses->apply_settings(std::move(p));
    return 0;
    MT_CATCH
}

int mt_add_magnet(mt_session *s, const uint8_t infohash[32], const char *save_path, uint64_t *handle_out, char *err,
                  size_t errlen) {
    MT_TRY
    std::lock_guard<std::mutex> g(s->mu);
    lt::add_torrent_params p;
    p.info_hashes = lt::info_hash_t(lt::sha256_hash(reinterpret_cast<const char *>(infohash)));
    p.save_path = save_path;
    // §5.3: at most 64 files, so 64 zero priorities cover every file the metadata can name.
    p.file_priorities.assign(64, lt::dont_download);
    if (s->i2p) {
        p.flags |= lt::torrent_flags::i2p_torrent;
        p.trackers = s->trackers;
    }
    p.flags &= ~lt::torrent_flags::auto_managed;
    p.flags &= ~lt::torrent_flags::paused;
    lt::error_code ec;
    lt::torrent_handle th = s->ses->add_torrent(std::move(p), ec);
    if (ec) {
        set_err(err, errlen, ec.message());
        return -1;
    }
    *handle_out = track(s, th);
    return 0;
    MT_CATCH
}

int mt_get_metadata(mt_session *s, uint64_t h, uint8_t *buf, size_t cap, size_t *len_out, char *err, size_t errlen) {
    MT_TRY
    std::lock_guard<std::mutex> g(s->mu);
    lt::torrent_handle th = find(s, h);
    if (!th.is_valid()) {
        set_err(err, errlen, "unknown handle");
        return -1;
    }
    std::shared_ptr<const lt::torrent_info> ti = th.torrent_file();
    if (!ti) {
        set_err(err, errlen, "no metadata yet");
        return -1;
    }
    lt::span<char const> info = ti->info_section();
    return copy_out(std::string(info.data(), static_cast<size_t>(info.size())), buf, cap, len_out);
    MT_CATCH
}

int mt_admit(mt_session *s, uint64_t h, char *err, size_t errlen) {
    MT_TRY
    std::lock_guard<std::mutex> g(s->mu);
    lt::torrent_handle th = find(s, h);
    std::shared_ptr<const lt::torrent_info> ti = th.is_valid() ? th.torrent_file() : nullptr;
    if (!ti) {
        set_err(err, errlen, "unknown handle or no metadata");
        return -1;
    }
    std::vector<lt::download_priority_t> prio(static_cast<size_t>(ti->num_files()), lt::default_priority);
    th.prioritize_files(prio);
    s->admitted.insert(h);
    th.resume();
    return 0;
    MT_CATCH
}

int mt_add_torrent(mt_session *s, const uint8_t *torrent, size_t len, const char *save_path, int32_t mode,
                   uint64_t *handle_out, char *err, size_t errlen) {
    MT_TRY
    std::lock_guard<std::mutex> g(s->mu);
    lt::error_code ec;
    auto ti = std::make_shared<lt::torrent_info>(lt::span<char const>(reinterpret_cast<const char *>(torrent),
                                                                      static_cast<std::ptrdiff_t>(len)),
                                                 ec, lt::from_span);
    if (ec) {
        set_err(err, errlen, ec.message());
        return -1;
    }
    lt::add_torrent_params p;
    p.ti = ti;
    p.save_path = save_path;
    p.flags &= ~lt::torrent_flags::auto_managed;
    p.flags &= ~lt::torrent_flags::paused;
    if (s->i2p) {
        p.flags |= lt::torrent_flags::i2p_torrent;
        p.trackers = s->trackers;
    }
    if (mode == MT_MODE_SEED || mode == MT_MODE_ADOPT) {
        // Sealed or adopted files are complete and are never written (MT-6): seed mode checks
        // pieces lazily as they are requested, upload mode forbids downloading into them.
        p.flags |= lt::torrent_flags::seed_mode;
        p.flags |= lt::torrent_flags::upload_mode;
    }
    lt::torrent_handle th = s->ses->add_torrent(std::move(p), ec);
    if (ec) {
        set_err(err, errlen, ec.message());
        return -1;
    }
    *handle_out = track(s, th);
    s->admitted.insert(*handle_out);
    return 0;
    MT_CATCH
}

int mt_pause(mt_session *s, uint64_t h, char *err, size_t errlen) {
    MT_TRY
    std::lock_guard<std::mutex> g(s->mu);
    lt::torrent_handle th = find(s, h);
    if (!th.is_valid()) {
        set_err(err, errlen, "unknown handle");
        return -1;
    }
    th.pause();
    return 0;
    MT_CATCH
}

int mt_resume(mt_session *s, uint64_t h, char *err, size_t errlen) {
    MT_TRY
    std::lock_guard<std::mutex> g(s->mu);
    lt::torrent_handle th = find(s, h);
    if (!th.is_valid()) {
        set_err(err, errlen, "unknown handle");
        return -1;
    }
    th.resume();
    return 0;
    MT_CATCH
}

int mt_remove(mt_session *s, uint64_t h, int32_t delete_files, char *err, size_t errlen) {
    MT_TRY
    std::lock_guard<std::mutex> g(s->mu);
    lt::torrent_handle th = find(s, h);
    if (!th.is_valid()) {
        set_err(err, errlen, "unknown handle");
        return -1;
    }
    s->by_hash.erase(th.info_hashes().v2);
    s->handles.erase(h);
    s->admitted.erase(h);
    s->ses->remove_torrent(th, delete_files != 0 ? lt::session_handle::delete_files : lt::remove_flags_t{});
    return 0;
    MT_CATCH
}

int mt_status(mt_session *s, uint64_t h, mt_status_t *out, char *err, size_t errlen) {
    MT_TRY
    std::lock_guard<std::mutex> g(s->mu);
    lt::torrent_handle th = find(s, h);
    if (!th.is_valid()) {
        set_err(err, errlen, "unknown handle");
        return -1;
    }
    lt::torrent_status st = th.status();
    std::memset(out, 0, sizeof(*out));
    if (st.errc) {
        out->state = MT_STATE_ERROR;
        copy_msg(out->error, st.errc.message());
    } else if (st.flags & lt::torrent_flags::paused) {
        out->state = MT_STATE_PAUSED;
    } else {
        switch (st.state) {
            case lt::torrent_status::downloading_metadata: out->state = MT_STATE_METADATA; break;
            case lt::torrent_status::checking_files:
            case lt::torrent_status::checking_resume_data: out->state = MT_STATE_CHECKING; break;
            case lt::torrent_status::finished: out->state = MT_STATE_FINISHED; break;
            case lt::torrent_status::seeding: out->state = MT_STATE_SEEDING; break;
            default: out->state = MT_STATE_DOWNLOADING; break;
        }
    }
    out->total = static_cast<uint64_t>(st.total_wanted);
    out->done = static_cast<uint64_t>(st.total_wanted_done);
    out->uploaded = static_cast<uint64_t>(st.all_time_upload);
    out->downloaded = static_cast<uint64_t>(st.all_time_download);
    out->upload_rate = static_cast<uint64_t>(std::max(0, st.upload_payload_rate));
    out->download_rate = static_cast<uint64_t>(std::max(0, st.download_payload_rate));
    out->peers = static_cast<uint32_t>(std::max(0, st.num_peers));
    out->seeds = static_cast<uint32_t>(std::max(0, st.num_seeds));
    return 0;
    MT_CATCH
}

int mt_piece_layers(mt_session *s, uint64_t h, uint8_t *buf, size_t cap, size_t *len_out, char *err, size_t errlen) {
    MT_TRY
    std::lock_guard<std::mutex> g(s->mu);
    lt::torrent_handle th = find(s, h);
    std::shared_ptr<lt::torrent_info> ti = th.is_valid() ? th.torrent_file_with_hashes() : nullptr;
    if (!ti) {
        set_err(err, errlen, "unknown handle or no metadata");
        return -1;
    }
    lt::entry layers(lt::entry::dictionary_t);
    const lt::file_storage &fs = ti->files();
    for (lt::file_index_t f : fs.file_range()) {
        lt::span<char const> layer = ti->piece_layer(f);
        // BEP 52: a file no longer than a piece has no layer (its root is its one piece hash);
        // libtorrent reports one anyway, and refuses it when the torrent is added again.
        if (layer.empty() || fs.file_size(f) <= ti->piece_length()) continue;
        lt::sha256_hash root = fs.root(f);
        layers[std::string(root.data(), root.size())] = std::string(layer.data(), static_cast<size_t>(layer.size()));
    }
    std::string out;
    lt::bencode(std::back_inserter(out), layers);
    return copy_out(out, buf, cap, len_out);
    MT_CATCH
}

size_t mt_pop_events(mt_session *s, mt_event *out, size_t max) {
    try {
        std::lock_guard<std::mutex> g(s->mu);
        std::vector<lt::alert *> alerts;
        s->ses->pop_alerts(&alerts);
        for (lt::alert *a : alerts) {
            mt_event ev{};
            auto id_of = [&](const lt::torrent_handle &th) -> uint64_t {
                auto it = s->by_hash.find(th.info_hashes().v2);
                return it == s->by_hash.end() ? 0 : it->second;
            };
            if (auto *m = lt::alert_cast<lt::metadata_received_alert>(a)) {
                ev.kind = MT_EV_METADATA;
                ev.handle = id_of(m->handle);
            } else if (auto *f = lt::alert_cast<lt::torrent_finished_alert>(a)) {
                ev.kind = MT_EV_FINISHED;
                ev.handle = id_of(f->handle);
                if (s->admitted.count(ev.handle) == 0) continue;
            } else if (auto *hf = lt::alert_cast<lt::hash_failed_alert>(a)) {
                ev.kind = MT_EV_HASH_FAILED;
                ev.handle = id_of(hf->handle);
                ev.piece = static_cast<uint64_t>(static_cast<int>(hf->piece_index));
            } else if (auto *pb = lt::alert_cast<lt::peer_ban_alert>(a)) {
                ev.kind = MT_EV_PEER_BANNED;
                ev.handle = id_of(pb->handle);
                copy_msg(ev.message, pb->message());
            } else if (auto *te = lt::alert_cast<lt::torrent_error_alert>(a)) {
                ev.kind = MT_EV_ERROR;
                ev.handle = id_of(te->handle);
                copy_msg(ev.message, te->message());
            } else if (auto *tr = lt::alert_cast<lt::tracker_error_alert>(a)) {
                // Operator diagnostics: the tracker's URL and the error, never a peer.
                std::fprintf(stderr, "misaka-torrentd: tracker error: %s\n", tr->message().c_str());
                continue;
            } else if (auto *tw = lt::alert_cast<lt::tracker_warning_alert>(a)) {
                std::fprintf(stderr, "misaka-torrentd: tracker warning: %s\n", tw->message().c_str());
                continue;
            } else if (auto *tl = lt::alert_cast<lt::torrent_log_alert>(a)) {
                std::string m = tl->log_message();
                if (m.find("announce") != std::string::npos || m.find("tracker") != std::string::npos ||
                    m.find("i2p") != std::string::npos || m.find("I2P") != std::string::npos)
                    std::fprintf(stderr, "misaka-torrentd: lt: %s\n", m.c_str());
                continue;
            } else if (auto *sl = lt::alert_cast<lt::log_alert>(a)) {
                std::string m = sl->log_message();
                if (m.find("i2p") != std::string::npos || m.find("I2P") != std::string::npos || m.find("SAM") != std::string::npos)
                    std::fprintf(stderr, "misaka-torrentd: lt-session: %s\n", m.c_str());
                continue;
            } else if (auto *ta = lt::alert_cast<lt::tracker_announce_alert>(a)) {
                std::fprintf(stderr, "misaka-torrentd: tracker announce: %s\n", ta->message().c_str());
                continue;
            } else if (auto *rp = lt::alert_cast<lt::tracker_reply_alert>(a)) {
                std::fprintf(stderr, "misaka-torrentd: tracker reply: %d peers\n", rp->num_peers);
                continue;
            } else if (auto *fe = lt::alert_cast<lt::file_error_alert>(a)) {
                ev.kind = MT_EV_ERROR;
                ev.handle = id_of(fe->handle);
                copy_msg(ev.message, fe->message());
            } else {
                continue;
            }
            if (ev.handle != 0) s->pending.push_back(ev);
        }
        size_t n = std::min(max, s->pending.size());
        for (size_t i = 0; i < n; ++i) {
            out[i] = s->pending.front();
            s->pending.pop_front();
        }
        return n;
    } catch (...) {
        return 0;
    }
}

}  // extern "C"
