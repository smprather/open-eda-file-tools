# Offline build test on AlmaLinux 8 (glibc 2.28, gcc 8.5).
#
#   docker build -f def_tools/docker/almalinux8.Dockerfile -t def-tools-almalinux8 .
#   docker run --rm --network none \
#     -v "${XDG_DATA_HOME:-$HOME/.local/share}/loadout:/opt/loadout:ro" \
#     def-tools-almalinux8
#
# The toolchain matches an air-gapped workstation: rustc/cargo and Python come
# from an Engineering Loadout install (rust + portable-python); gcc is the
# system baseline needed to compile zstd-sys's vendored C sources. The compile
# and example run are offline: no network at run time, and cargo resolves every
# crate from vendor/cargo.

FROM almalinux:8

RUN dnf install -y gcc make zlib-devel tar gzip \
    && dnf clean all

# The source is read-only and the scratch directory is owned by an
# unprivileged user, so the "nothing is written to the checkout" claim is
# enforced by permissions rather than discipline.
COPY . /src
RUN chmod -R a-w /src \
    && useradd -m runner \
    && mkdir -p /run/work \
    && chown -R runner:runner /run

USER runner
WORKDIR /run/work
CMD ["/bin/bash", "/src/def_tools/docker/run-offline-example.sh"]
