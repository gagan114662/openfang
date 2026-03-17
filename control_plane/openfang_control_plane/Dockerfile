FROM elixir:1.17 AS build

WORKDIR /app

ENV MIX_ENV=prod

# Build deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential git ca-certificates \
  && rm -rf /var/lib/apt/lists/*

RUN mix local.hex --force && mix local.rebar --force

COPY mix.exs mix.lock ./
COPY config config
RUN mix deps.get --only prod
RUN mix deps.compile

COPY lib lib
COPY priv priv
COPY rel rel

RUN mix compile
RUN mix release

FROM debian:bookworm-slim AS runtime

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates openssl \
  && rm -rf /var/lib/apt/lists/*

ENV LANG=C.UTF-8

COPY --from=build /app/_build/prod/rel/openfang_control_plane ./
COPY entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

ENV OFCP_DB_PATH=/app/data/ofcp.sqlite3

ENTRYPOINT ["/app/entrypoint.sh"]

