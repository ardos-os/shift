## WARNING: This PKGBUILD is not meant to be used with arch linux directly, this is a build recipe for Ardos OS only
##          There's no guarantee whatsoever that this will work on normal linux distributions, use this at your own risk
# Maintainer: Tiago <tiago@ardos.local>

pkgname=shift
pkgver=0.1.0_alpha
_cargo_pkgver=0.1.0-alpha
pkgrel=1
pkgdesc="GUI-first replacement for Linux TTY session management"
arch=('x86_64')
url="https://github.com/ardos-os/shift"
license=('MIT')
options=('!lto')
depends=(
    'glibc'
    'libgcc'
    'libstdc++'
    'mesa'
    'libevdev'
    'mtdev'
    'libwacom'
    'skia'
)
makedepends=('rust' 'clang' 'pkgconf' 'git')
source=()
sha256sums=()

_cargo_target_dir() {
    printf '%s\n' "${BUILDDIR}/cargo-target/target"
}

build() {
    export CARGO_TARGET_DIR="$(_cargo_target_dir)"
    export CARGO_NET_GIT_FETCH_WITH_CLI=true
    export SKIA_SOURCE_DIR="/usr/lib/skia/source"
    export SKIA_LIBRARY_SEARCH_PATH="/usr/lib"
    export SKIA_BUILD_DEFINES="$(< /usr/lib/skia/skia-defines.txt)"
    export SKIA_LINK_SHARED=1
    chmod +x "${startdir}/ardos-linker"
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="${startdir}/ardos-linker"

    cargo build \
        --manifest-path "${startdir}/Cargo.toml" \
        --release \
        -p shift
}

package() {
    install -Dm755 \
        "$(_cargo_target_dir)/release/shift" \
        "${pkgdir}/ardos/services/shift/shift"
}
