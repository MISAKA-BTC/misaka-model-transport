// Phase A1's exit gate (RFC-0001 §9): the canonical infohash of a bundle equals libtorrent's.
//
//   c++ -std=c++17 -O2 scripts/golden_libtorrent.cpp -I<lt>/include -L<lt>/lib -ltorrent-rasterbar \
//       -o golden_libtorrent
//   ./golden_libtorrent <bundle-dir>      # prints libtorrent's v2 infohash and the info bytes' size
//
// <bundle-dir> is named after the bundle's title. libtorrent builds a v2-only torrent of the same
// files at the piece length rule v1 picks; `misaka-torrent inspect <bundle-dir>` must print the
// same infohash. No file may carry an attribute (an executable bit becomes BEP 47 `attr`).

#include <libtorrent/create_torrent.hpp>
#include <libtorrent/hex.hpp>
#include <libtorrent/hasher.hpp>
#include <libtorrent/bencode.hpp>
#include <libtorrent/file_storage.hpp>
#include <libtorrent/version.hpp>

#include <cstdint>
#include <iostream>
#include <iterator>
#include <string>
#include <vector>

namespace lt = libtorrent;

static int piece_length(std::int64_t total) {
    std::int64_t n = (total + 4095) / 4096, p = 1;
    while (p < n) p <<= 1;
    if (p < (1 << 20)) p = 1 << 20;
    if (p > (16 << 20)) p = 16 << 20;
    return static_cast<int>(p);
}

int main(int argc, char **argv) {
    if (argc != 2) {
        std::cerr << "usage: golden_libtorrent <bundle-dir>\n";
        return 2;
    }
    std::string dir = argv[1];
    while (dir.size() > 1 && dir.back() == '/') dir.pop_back();
    std::string parent = dir.substr(0, dir.find_last_of('/'));
    lt::file_storage fs;
    lt::add_files(fs, dir);
    lt::create_torrent ct(fs, piece_length(fs.total_size()), lt::create_torrent::v2_only);
    lt::set_piece_hashes(ct, parent);
    lt::entry e = ct.generate();
    std::vector<char> info;
    lt::bencode(std::back_inserter(info), e["info"]);
    lt::hasher256 h;
    h.update(info);
    std::cout << "libtorrent " << LIBTORRENT_VERSION << "  infohash " << lt::aux::to_hex(h.final()) << "  info "
              << info.size() << " bytes\n";
    return 0;
}
