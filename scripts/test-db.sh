#!/usr/bin/env bash
# Manage the throwaway Postgres the integration tests run against.
#
# The tests need a database with the *real* Onyx DDL — every column, every foreign
# key — because that is what makes them evidence about production rather than
# evidence about a hand-written approximation. `tests/fixtures/onyx_schema.sql` is
# a captured `pg_dump` of the live schema; `seed.sql` adds rows shaped around the
# defects being regression-tested.
#
# Usage:
#   scripts/test-db.sh up       # start, create schema, seed, print the DSN
#   scripts/test-db.sh reset    # drop and re-seed without restarting the container
#   scripts/test-db.sh dsn      # print the DSN
#   scripts/test-db.sh stop     # stop the container (used to test PG-down handling)
#   scripts/test-db.sh start    # start it again
#   scripts/test-db.sh down     # remove it entirely
#
# Then:
#   export OVIS_TEST_DATABASE_URL="$(scripts/test-db.sh dsn)"
#   cargo test --workspace
#
# Integration tests that need a database skip themselves when
# OVIS_TEST_DATABASE_URL is unset, so `cargo test` works without Docker.
set -euo pipefail

NAME="${OVIS_TEST_PG_NAME:-ovis-test-pg}"
PORT="${OVIS_TEST_PG_PORT:-55433}"
PASSWORD="ovis-test-only"
IMAGE="postgres:15-alpine"
DSN="postgres://postgres:${PASSWORD}@127.0.0.1:${PORT}/postgres"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

wait_ready() {
  for _ in $(seq 1 60); do
    if docker exec "$NAME" pg_isready -U postgres -q 2>/dev/null; then return 0; fi
    sleep 0.5
  done
  echo "$NAME did not become ready" >&2
  return 1
}

apply_fixtures() {
  # `pg_trgm` matches production, where the OVIS support indexes need it.
  docker exec -i "$NAME" psql -U postgres -d postgres -q -v ON_ERROR_STOP=1 \
    -c "CREATE EXTENSION IF NOT EXISTS pg_trgm;"
  docker exec -i "$NAME" psql -U postgres -d postgres -q -v ON_ERROR_STOP=1 \
    < "$ROOT/tests/fixtures/onyx_schema.sql"
  docker exec -i "$NAME" psql -U postgres -d postgres -q -v ON_ERROR_STOP=1 \
    < "$ROOT/tests/fixtures/seed.sql"
  # The same support indexes production has, so the tests exercise the same plans.
  docker exec -i "$NAME" psql -U postgres -d postgres -q -v ON_ERROR_STOP=1 \
    < "$ROOT/ops/onyx_indexes.sql"
}

case "${1:-up}" in
  up)
    if docker inspect "$NAME" >/dev/null 2>&1; then
      docker start "$NAME" >/dev/null
    else
      docker run -d --name "$NAME" \
        -e POSTGRES_PASSWORD="$PASSWORD" \
        -e POSTGRES_DB=postgres \
        -p "${PORT}:5432" \
        "$IMAGE" >/dev/null
    fi
    wait_ready
    apply_fixtures
    echo "$DSN"
    ;;
  reset)
    wait_ready
    docker exec -i "$NAME" psql -U postgres -d postgres -q -v ON_ERROR_STOP=1 \
      -c "DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public; DROP SCHEMA IF EXISTS ovis CASCADE;"
    apply_fixtures
    echo "$DSN"
    ;;
  dsn)   echo "$DSN" ;;
  stop)  docker stop "$NAME" >/dev/null && echo "stopped $NAME" ;;
  start) docker start "$NAME" >/dev/null && wait_ready && echo "started $NAME" ;;
  down)  docker rm -f "$NAME" >/dev/null 2>&1 && echo "removed $NAME" || true ;;
  *)
    echo "usage: $0 {up|reset|dsn|stop|start|down}" >&2
    exit 2
    ;;
esac
