# Bundled boot assets

The graphical client packages include the following independently distributed network boot components. They are stored as separate files under `assets/boot` (Android: `assets/boot`) and are not linked into Bootah.

| Component | Version / source | License |
| --- | --- | --- |
| iPXE (`ipxe-*.efi`, `snponly-x64.efi`, `undionly.kpxe`) | Prebuilt upstream binaries; exact URL and SHA-256 are recorded in `assets/boot/sources.json` | Primarily GPL-2.0-or-later; some files use GPL-compatible licenses. The precise license set varies by build. See [upstream licensing](https://www.ipxe.org/licensing), [GPL-2.0 text](https://www.gnu.org/licenses/old-licenses/gpl-2.0.html), and source at https://github.com/ipxe/ipxe. |
| Signed iPXE (`signed-*-ipxe.efi`) and iPXE shim (`signed-*-ipxe-shim.efi`) | iPXE v2.0.0 release; archive and component hashes pinned in `assets/signed-ipxe.json` | iPXE: GPL-2.0-or-later and compatible component licenses; shim: BSD-2-Clause and component licenses. Independently distributed binaries, not linked into Bootah. Sources: https://github.com/ipxe/ipxe/tree/v2.0.0 and https://github.com/ipxe/shim. |
| wimboot (`wimboot`, `wimboot-arm64`) | Upstream release 2.9.0; URLs and SHA-256 are recorded in `assets/boot/sources.json` | GNU GPL-2.0-or-later. See https://www.gnu.org/licenses/old-licenses/gpl-2.0.html, https://ipxe.org/wimboot and source at https://github.com/ipxe/wimboot. |

Power assets: `grub-powerpc.elf` is a separate GNU GRUB powerpc-ieee1275
network loader (GPL-3.0-or-later); corresponding source and build instructions
are indexed in `tools/power-boot/README.md`. `bootah-media-ppc64le` is the MIT
helper from `tools/linux-media`, statically linked with GNU libc
(LGPL-2.1-or-later). Preserve GNU libc notices and corresponding source/rebuild
materials when redistributing that binary. Neither is linked into Bootah.

These are binary distributions. Corresponding source is available from the upstream repositories above. Preserve the upstream copyright and license notices when redistributing. iPXE recommends generating a `.licence` report for the exact build; Bootah currently consumes upstream prebuilt binaries, so the upstream build-specific report should be consulted for an exact component inventory.

# Other direct open-source dependencies

- Rust core dependencies are declared in `crates/netboot-core/Cargo.toml`; the crates use their respective upstream licenses, commonly MIT and/or Apache-2.0. `nfsserve` is BSD-3-Clause; its license text is in `nfsserve.txt`.
- Avalonia and Avalonia.Themes.Fluent are MIT-licensed; Tmds.DBus.Protocol is MIT-licensed. Versions are declared in `apps/desktop/Netboot.Desktop.csproj`. MIT text: https://opensource.org/license/mit.
- Rust crates commonly use MIT and/or Apache-2.0; consult each locked crate's manifest for its exact license. Apache-2.0 text: https://www.apache.org/licenses/LICENSE-2.0.
- Android uses the AndroidX/Android Gradle dependency set declared in `apps/android`; consult package metadata for the exact transitive dependency versions and notices.
- The project also includes `NetBSD-mopd.txt`, `ods2.txt`, and `bootah-linux-media.txt` for the corresponding components.

This notice is an index, not a replacement for the license text or source-offer requirements that apply to each specific binary and release.
