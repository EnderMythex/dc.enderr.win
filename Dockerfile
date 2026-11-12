# Two images from one build:
#   docker build --target panel -t dc-panel .
#   docker build --target bot   -t dc-bot .
FROM rust:1-slim-bookworm AS build
WORKDIR /app
# Only the sources: editing a bot.toml does not trigger a new compilation.
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# Docker keeps the downloaded crates and the compiled code between deploys
# (BuildKit cache mounts, outside the images): only the code that changed is
# compiled again, in seconds instead of minutes. `locked`: the panel and bot
# builds, started together, wait for each other instead of compiling twice.
# The cache is about 1.1 GB; old dependency versions pile up after updates,
# so past 2.5 GB it starts over (that one build takes ~3 minutes).
RUN --mount=type=cache,id=dc-cargo,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=dc-target,target=/app/target,sharing=locked \
    cargo build --release --locked -p dc-panel -p dc-bot \
    && cp target/release/dc-panel target/release/dc-bot /usr/local/bin/ \
    && if [ "$(du -sm target | cut -f1)" -gt 2500 ]; then cargo clean; fi

FROM debian:bookworm-slim AS base
# uid 10001 must be able to write the bots folder (see README). /bots exists
# in the image so a named volume mounted there starts owned by that user.
RUN useradd --system --uid 10001 app \
    && mkdir /bots && chown app /bots
# Bot definitions (bot.toml), copied into /bots at startup when
# BOTS_SEED_DIR points here (compose.coolify.yaml). Secrets are excluded by
# .dockerignore and never part of the image.
COPY bots /opt/dc/bots
USER app

FROM base AS bot
COPY --from=build /usr/local/bin/dc-bot /usr/local/bin/dc-bot
ENV BOT_CONFIG_DIR=/config
CMD ["dc-bot"]

FROM base AS panel
USER root
RUN mkdir /data && chown app /data
USER app
COPY --from=build /usr/local/bin/dc-panel /usr/local/bin/dc-panel
ENV BIND_ADDR=0.0.0.0:8080 \
    DATABASE_URL=sqlite:///data/panel.db
VOLUME /data
EXPOSE 8080
CMD ["dc-panel"]
