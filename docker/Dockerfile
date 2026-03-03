# envpod Docker Test Environment
# 
# Try envpod without installing anything on your host.
#
# Build:
#   docker build -t envpod-demo .
#
# Run:
#   docker run -it --privileged --cgroupns=host \
#     -v /tmp/envpod-test:/var/lib/envpod \
#     -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
#     envpod-demo
#
# Then inside the container:
#   envpod init test -c /opt/envpod/examples/basic-internet.yaml
#   envpod run test -- bash

FROM ubuntu:24.04

LABEL maintainer="mark@envpod.dev"
LABEL description="envpod test environment — zero-trust governance for AI agents"
LABEL org.opencontainers.image.source="https://github.com/markamo/envpod-ce"

# System dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    iptables \
    iproute2 \
    net-tools \
    dnsutils \
    tcpdump \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Install envpod
RUN cd /tmp \
    && curl -fsSL https://github.com/markamo/envpod-ce/releases/latest/download/envpod-linux-x86_64.tar.gz | tar xz \
    && cd envpod-*-linux-x86_64 \
    && ./install.sh \
    && cp -r examples /opt/envpod-examples \
    && cd / \
    && rm -rf /tmp/envpod-*

# Copy examples to a known location
RUN mkdir -p /opt/envpod && mv /opt/envpod-examples /opt/envpod/examples

# Startup script
COPY entrypoint.sh /opt/envpod/entrypoint.sh
RUN chmod +x /opt/envpod/entrypoint.sh

ENTRYPOINT ["/opt/envpod/entrypoint.sh"]
CMD ["bash"]
