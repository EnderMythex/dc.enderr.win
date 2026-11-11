# Two images from one build:
#   docker build --target panel -t dc-panel .
#   docker build --target bot   -t dc-bot .
FROM rust:1-bookworm AS build
WORKDIR /app
# Only the sources: editing a bot.toml does not trigger a new compilation.
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --release --locked -p dc-panel -p dc-bot

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
COPY --from=build /app/target/release/dc-bot /usr/local/bin/dc-bot
ENV BOT_CONFIG_DIR=/config
CMD ["dc-bot"]

FROM base AS panel
USER root
RUN mkdir /data && chown app /data
USER app
COPY --from=build /app/target/release/dc-panel /usr/local/bin/dc-panel
ENV BIND_ADDR=0.0.0.0:8080 \
    DATABASE_URL=sqlite:///data/panel.db
VOLUME /data
EXPOSE 8080
CMD ["dc-panel"]
