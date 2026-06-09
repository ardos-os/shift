FROM archlinux:multilib-devel

RUN pacman -Sy --noconfirm \
    rust \
    clang \
    pkgconf \
    git \
    python \
    cmake \
    ninja \
 && pacman -Scc --noconfirm
