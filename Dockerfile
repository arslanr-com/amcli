# syntax=docker/dockerfile:1

# amcli as a read-only viewer over a model, for a demo deployment behind a
# reverse proxy (Coolify). Two stages: build the binary and, with it, the demo
# model; then carry both into an image that has nothing else in it.
#
# See docs/container.md for what to set and what is served.

# ---- build -------------------------------------------------------------------
# Pinned to the workspace's rust-version. `--locked` means the lockfile in the
# repository is the one that is built, so two builds of the same commit agree.
FROM rust:1.90-bookworm AS build

# `--version` carries the commit, which build.rs takes from git. There is no
# .git here (see .dockerignore), so the build passes it in; empty is fine and
# becomes "unknown build".
ARG AMCLI_BUILD=""
ENV AMCLI_BUILD=$AMCLI_BUILD

WORKDIR /src
COPY . .
RUN cargo build --release --locked -p amcli-cli

# The model the container serves, generated from the batch in deploy/demo/ by
# the binary that was just built. The seed is pinned, so this is the same file
# every time; nothing is downloaded and nothing secret is baked in.
RUN mkdir -p /out && sh deploy/demo/build-model.sh /src/target/release/amcli /out/demo.archimate

# ---- runtime -----------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# curl is the healthcheck's client; the fonts are what PNG export draws labels
# with — without them a rendered view comes out with no text on it.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/*

# A fixed uid, so a mounted volume's ownership is predictable.
RUN useradd --system --uid 10001 --create-home --home-dir /app --shell /usr/sbin/nologin amcli

COPY --from=build /src/target/release/amcli /usr/local/bin/amcli
COPY --from=build /out/demo.archimate /app/model/demo.archimate

# Read-only by construction: the viewer serves GET and nothing else, and the
# process never writes the file. The model is owned by root and world-readable
# so the unprivileged user cannot change what it is showing.
RUN chmod -R a+rX /app/model

# The model to serve, and where. AMCLI_WEB_BIND has to be the wildcard for the
# proxy to reach the port; AMCLI_WEB_ALLOW_HOST is then what keeps any other
# origin out, so it names the host this demo answers to. Override it in Coolify
# if the domain changes. None of these is a secret.
ENV AMCLI_MODEL=/app/model/demo.archimate \
    AMCLI_WEB_BIND=0.0.0.0 \
    AMCLI_WEB_PORT=3000 \
    AMCLI_WEB_ALLOW_HOST=amcli.arslanr.com \
    RUST_BACKTRACE=0

# The internal port. Coolify maps its proxy onto this.
EXPOSE 3000

USER 10001:10001
WORKDIR /app

# /api/status is the viewer's own answer about the model it is holding — it is
# 200 only once the model has been parsed and served, so it is a real readiness
# signal rather than a socket that happens to be open. The Host is loopback,
# which is allowed whatever AMCLI_WEB_ALLOW_HOST says.
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
  CMD curl -fsS http://127.0.0.1:3000/api/status || exit 1

# --no-open: there is no browser to hand the URL to. Everything else comes
# from the environment above.
CMD ["amcli", "web", "--no-open"]
