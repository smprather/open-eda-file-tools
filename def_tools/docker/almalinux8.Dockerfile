# Offline build test on AlmaLinux 8 (glibc 2.28, gcc 8.5).
#
#   docker build -f def_tools/docker/almalinux8.Dockerfile -t def-tools-almalinux8 .
#   docker run --rm --network none def-tools-almalinux8
#
# rustc and cargo are assumed to exist on the offline system; this image
# installs them from the AlmaLinux module only to provide that baseline. The
# compile and example run are offline: no network at run time, and cargo
# resolves every crate from vendor/cargo.

FROM almalinux:8

RUN dnf install -y gcc make zlib-devel tar gzip wget rust cargo \
    && dnf clean all

# python-build-standalone targets glibc 2.17, so this interpreter runs on
# AlmaLinux 8. Pin the URL and checksum for a reproducible toolchain.
ARG PYTHON_URL=https://releases.astral.sh/github/python-build-standalone/releases/download/20261013/cpython-3.15.0%2B20261013-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz
ARG PYTHON_SHA256=69740dcd9b3480b3d53760f2d3b75582adb14fa54434628dea32480ddede542a
RUN wget -q "$PYTHON_URL" -O /tmp/python.tar.gz \
    && echo "$PYTHON_SHA256  /tmp/python.tar.gz" | sha256sum -c - \
    && mkdir -p /opt/python \
    && tar -xzf /tmp/python.tar.gz -C /opt/python --strip-components=1 \
    && rm /tmp/python.tar.gz

ENV PYTHON=/opt/python/bin/python3.15

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
