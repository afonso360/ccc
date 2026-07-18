FROM rust:1.96.0-bookworm@sha256:5e2214abe154fe26e39f64488952e5c991eeed1d6d6da7cc8381ae83927f0cfc

ARG DEBIAN_FRONTEND=noninteractive

RUN sed -i \
        -e 's|http://deb.debian.org/debian-security|https://snapshot.debian.org/archive/debian-security/20260714T000000Z|g' \
        -e 's|http://deb.debian.org/debian|https://snapshot.debian.org/archive/debian/20260714T000000Z|g' \
        /etc/apt/sources.list.d/debian.sources

RUN apt-get -o Acquire::Check-Valid-Until=false update \
    && apt-get install -y --no-install-recommends \
        binutils-aarch64-linux-gnu=2.40-2 \
        binutils-riscv64-linux-gnu=2.40-2 \
        gcc-aarch64-linux-gnu=4:12.2.0-3 \
        gcc-riscv64-linux-gnu=4:12.2.0-5 \
        libc6-dev-arm64-cross=2.36-8cross1 \
        libc6-dev-riscv64-cross=2.36-8cross1 \
        gdb-multiarch=13.1-3 \
        qemu-user=1:7.2+dfsg-7+deb12u18+b3 \
        openssl \
    && rm -rf /var/lib/apt/lists/*

RUN test "$(dpkg-query -W -f='${Version}' binutils-aarch64-linux-gnu)" = 2.40-2 \
    && test "$(dpkg-query -W -f='${Version}' binutils-riscv64-linux-gnu)" = 2.40-2 \
    && test "$(dpkg-query -W -f='${Version}' gcc-aarch64-linux-gnu)" = 4:12.2.0-3 \
    && test "$(dpkg-query -W -f='${Version}' gcc-riscv64-linux-gnu)" = 4:12.2.0-5 \
    && test "$(dpkg-query -W -f='${Version}' libc6-dev-arm64-cross)" = 2.36-8cross1 \
    && test "$(dpkg-query -W -f='${Version}' libc6-dev-riscv64-cross)" = 2.36-8cross1 \
    && test "$(dpkg-query -W -f='${Version}' gdb-multiarch)" = 13.1-3 \
    && test "$(dpkg-query -W -f='${Version}' qemu-user)" = 1:7.2+dfsg-7+deb12u18+b3 \
    && test "$(aarch64-linux-gnu-gcc -dumpmachine)" = aarch64-linux-gnu \
    && test "$(riscv64-linux-gnu-gcc -dumpmachine)" = riscv64-linux-gnu \
    && test -f /usr/aarch64-linux-gnu/lib/ld-linux-aarch64.so.1 \
    && test -f /usr/riscv64-linux-gnu/lib/ld-linux-riscv64-lp64d.so.1
