# Historical Debian package provenance

`dist/tgrep_1.0.0_amd64.deb` was added to Git by commit
`d5787093315c2c93d46020cbf4d49e92d22149a9` on 2026-09-26. In that commit's
tree, `Cargo.toml` declared `AGPL-3.0-only`, the root `LICENSE` was the GNU
Affero General Public License version 3, and the Debian control file also
declared AGPL-3.0. The project owner confirmed SRSL-1.1 as the intended source
license. The root `LICENSE` is now an exact copy of the authoritative
[`LicenseRef-SRSL-1.1.txt`](https://github.com/SWORDIntel/QIHSE/blob/main/LICENSES/LicenseRef-SRSL-1.1.txt)
document (SHA-256 `45a702f6848465126b4d7da6f98a2e8b6bc1390dd9829ef8d4270db40756ee32`).
Current `Cargo.toml`, source-file SPDX headers, and source-header guidance
declare `LicenseRef-SRSL-1.1`. The current package staging `LICENSE` is copied
from the root document, and `build_deb.sh` checks its heading against Cargo
metadata. The old AGPL package remains historical and must not be presented as
a build of current source.

The Git history associates the package with the 2026-09-26 packaging revision,
but does not include a build log, source revision identifier inside the binary,
or reproducible-build attestation. The exact source tree used to produce the
ELF therefore cannot be established from the artifact alone. The ELF bytes
inside the package do match the separately committed staging binary at
`packaging/deb/root/usr/bin/tgrep`; this confirms package-content identity, not
which exact source state produced that executable.

The packaged ELF requires `GLIBC_2.39` symbols, as reported by
`readelf --version-info`; the package's original `Depends` field said only
`libc6 (>= 2.31)`. This artifact's dependency metadata has been corrected to
`libc6 (>= 2.39)`. New packages should continue deriving dependencies from the
staged ELF with `dpkg-shlibdeps`.

The historical AGPL artifact remains distinct from current SRSL-1.1 source. Its
embedded AGPL license and original release metadata are retained as historical
artifact contents; only its incorrect libc dependency floor was corrected.
