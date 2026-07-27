#!/usr/bin/env bash
# Refresh tests/fixtures/onyx_schema.sql from a live Onyx database.
#
# The integration tests run against the real DDL rather than a hand-written
# approximation, so that a query which passes the tests passes in production for
# the same reason — same columns, same types, same foreign keys. Re-run this after
# an Onyx upgrade; the startup schema probe also reports when it has gone stale.
#
# Usage:
#   scripts/capture-onyx-schema.sh gamma > tests/fixtures/onyx_schema.sql
#
# Requires ssh access to a host running the `onyx-postgres` container.
set -euo pipefail

HOST="${1:-gamma}"
CONTAINER="${OVIS_PG_CONTAINER:-onyx-postgres}"
DB_USER="${OVIS_PG_USER:-postgres}"
DB_NAME="${OVIS_PG_DB:-postgres}"

# Every table the OVIS data layer reads, plus every foreign-key child of
# `document` that the cascading delete has to clear, plus the tables those
# children reference so the dump restores standalone.
TABLES=(
  document
  document_by_connector_credential_pair
  connector
  connector_credential_pair
  credential
  index_attempt
  index_attempt_errors
  background_error
  tag
  document__tag
  search_settings
  chunk_stats
  document_retrieval_feedback
  kg_entity
  kg_entity_extraction_staging
  kg_relationship
  kg_relationship_extraction_staging
  kg_entity_type
  kg_relationship_type
  kg_relationship_type_extraction_staging
  hierarchy_node
  persona__document
  opensearch_document_migration_record
)

table_args=""
for t in "${TABLES[@]}"; do
  table_args+=" -t public.${t}"
done

dump_flags="--schema-only --no-owner --no-privileges --no-comments"

# Two passes. Sequences come first and separately because Onyx creates some of
# them standalone rather than owned by their column, and `pg_dump -t <table>`
# carries only the owned ones — without them the restore fails on
# `DEFAULT nextval(...)`. Emitting sequences before tables also makes the output
# order valid on replay.
remote_cmd="pg_dump -U ${DB_USER} -d ${DB_NAME} ${dump_flags} -t 'public.*_seq'; \
pg_dump -U ${DB_USER} -d ${DB_NAME} ${dump_flags}${table_args}"

ssh "$HOST" "docker exec -i ${CONTAINER} sh -c \"${remote_cmd}\"" |
  # Strip SET/ownership noise that varies by server version and that a test
  # database does not need. Duplicate statements (the two passes overlap on owned
  # sequences) are harmless: every one is IF-NOT-EXISTS-safe or idempotent on a
  # fresh database, and `scripts/test-db.sh` always starts from a fresh schema.
  grep -v -E '^(SET |SELECT pg_catalog\.set_config|--|\\connect)' |
  grep -v -E '^(CREATE EXTENSION|COMMENT ON EXTENSION)' |
  cat -s
