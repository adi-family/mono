# A throwaway Linux "node" for testing docs/fleet.md §6 end to end.
#
# Real systemd as PID 1, because the whole point is to exercise adi-core's systemd back-end —
# the unit files, `systemctl --user`, and lingering. A container that fakes supervision would
# test nothing that has not already been unit-tested on the host.
#
# Run it privileged with a cgroup mount; see run.sh.
FROM debian:bookworm

ENV DEBIAN_FRONTEND=noninteractive
ENV container=docker

# libpam-systemd + dbus-user-session are the two that matter and are easy to miss: without
# pam_systemd, logind never creates $XDG_RUNTIME_DIR, so `systemd --user` refuses to start and
# every `systemctl --user` fails with "Failed to connect to bus" — which looks exactly like a
# bug in whatever you are testing.
RUN apt-get update && apt-get install -y --no-install-recommends \
        systemd systemd-sysv dbus dbus-user-session libpam-systemd \
        systemd-resolved \
        libcap2-bin sudo \
        ca-certificates curl unzip iproute2 procps dnsutils \
    && rm -rf /var/lib/apt/lists/*

# bun, because a dashboard is a pair of bun services (docs/fleet.md §4). The node package does
# not ship it — this is the prerequisite that has to be documented or installed, and testing the
# dashboard path is how we find out which.
RUN curl -fsSL https://bun.sh/install | bash -s -- bun-v1.1.38 \
    && ln -sf /root/.bun/bin/bun /usr/local/bin/bun \
    && chmod -R a+rx /root/.bun /root

# systemd in a container: drop the units that need real hardware or a real network stack, so
# boot reaches a running system instead of hanging on a device that will never appear.
RUN find /etc/systemd/system /lib/systemd/system \
        -path '*.wants/*' \
        \( -name '*udev*' -o -name '*getty*' -o -name '*console*' \) \
        -exec rm -f {} \; 2>/dev/null || true

# The node runs as an ordinary user — that is the property under test (no root daemon, no
# root-owned files in ~/.adi). Give it a home and a real login shell.
RUN useradd -m -s /bin/bash adi \
    && echo 'adi ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/adi \
    && chmod 0440 /etc/sudoers.d/adi \
    && loginctl enable-linger adi 2>/dev/null || true

STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
