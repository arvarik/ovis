-- ============================================================================
-- OVIS support indexes for the Onyx Postgres database
-- ============================================================================
--
-- These indexes are what make the OVIS list/search path meet its performance
-- budgets (see docs/operations.md). They are additive and
-- read-only in effect: no Onyx table is altered, no data is written.
--
-- OVIS NEVER APPLIES THIS FILE AUTOMATICALLY. Onyx owns this database; we do
-- not silently run DDL against someone else's schema. Run it by hand, off-peak.
-- The OVIS startup probe reports which of these indexes are missing, and
-- GET /api/v1/system/health lists them under `missing_indexes` as a
-- performance warning (never an error).
--
-- Usage (from a host that can reach the DB directly on :5433 — NOT pgbouncer):
--     psql "$DATABASE_URL" -f ops/onyx_indexes.sql
--
-- Every statement uses CREATE INDEX CONCURRENTLY, so writes keep flowing while
-- the indexes build. CONCURRENTLY cannot run inside a transaction block, so do
-- not wrap this file in BEGIN/COMMIT and do not run it with `psql -1`.
--
-- Measured on gamma (2026-07-26, 1,652,044 rows in public.document):
--   before: default list page = 965 ms  (parallel seq scan + 150 MB external merge sort)
--   after:  0.6 ms (index-served; the full acceptance run is gated by ovis-bench)
--
-- Expected total size: ~1-2 GB (the two GIN trigram indexes dominate).
-- All of it is reversible: DROP INDEX CONCURRENTLY <name>;  (see bottom of file)
-- ============================================================================

-- Required for the ILIKE '%term%' title/URL filter to be index-served instead
-- of sequentially scanning 1.65M rows on every keystroke.
CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- ---------------------------------------------------------------------------
-- 1. Default sort + keyset pagination.
--    Matches ORDER BY COALESCE(doc_updated_at, last_modified) DESC, id DESC
--    exactly, including the keyset tuple comparison. This is the single most
--    important index in this file: without it the list path sorts the whole
--    table on every request.
-- ---------------------------------------------------------------------------
CREATE INDEX CONCURRENTLY IF NOT EXISTS ix_ovis_document_updated
    ON public.document ((COALESCE(doc_updated_at, last_modified)) DESC, id DESC);

-- ---------------------------------------------------------------------------
-- 2. chunk_min / chunk_max filters (the "stubs" and "heavy pages" presets)
--    and sort=chunks_asc keyset pagination. `chunk_count` is nullable
--    (49k rows on gamma), so both chunk sorts are NULLS LAST and the keyset
--    predicate handles the null tail explicitly.
-- ---------------------------------------------------------------------------
CREATE INDEX CONCURRENTLY IF NOT EXISTS ix_ovis_document_chunk_count
    ON public.document (chunk_count, id DESC);

-- sort=chunks_desc: ORDER BY chunk_count DESC NULLS LAST, id DESC
CREATE INDEX CONCURRENTLY IF NOT EXISTS ix_ovis_document_chunk_count_desc
    ON public.document (chunk_count DESC NULLS LAST, id DESC);

-- sort=boost_desc: ORDER BY boost DESC, id DESC. Very low cardinality (almost
-- every row is boost=0), which is exactly why the index matters: it lets
-- LIMIT 50 stop after 50 index entries instead of sorting the whole table.
CREATE INDEX CONCURRENTLY IF NOT EXISTS ix_ovis_document_boost
    ON public.document (boost DESC, id DESC);

-- ---------------------------------------------------------------------------
-- 3. Title / URL substring search (search= parameter).
-- ---------------------------------------------------------------------------
CREATE INDEX CONCURRENTLY IF NOT EXISTS ix_ovis_document_semantic_id_trgm
    ON public.document USING gin (semantic_id gin_trgm_ops);

CREATE INDEX CONCURRENTLY IF NOT EXISTS ix_ovis_document_id_trgm
    ON public.document USING gin (id gin_trgm_ops);

-- ---------------------------------------------------------------------------
-- 4. Connector attribution lateral (dcc lookup by document id).
--    document_by_connector_credential_pair's PK is (id, connector_id,
--    credential_id) so a leading-column index already exists; this narrower
--    one keeps the lateral an index-only scan and is cheap.
-- ---------------------------------------------------------------------------
CREATE INDEX CONCURRENTLY IF NOT EXISTS ix_ovis_dcc_by_doc
    ON public.document_by_connector_credential_pair (id);

-- ---------------------------------------------------------------------------
-- 5. Tag facet joins (/api/v1/tags and per-document tag fetch).
--    document__tag's PK is (document_id, tag_id); the reverse direction is what
--    the facet aggregation needs.
-- ---------------------------------------------------------------------------
CREATE INDEX CONCURRENTLY IF NOT EXISTS ix_ovis_document_tag_by_tag
    ON public.document__tag (tag_id);

-- ---------------------------------------------------------------------------
-- Verification
-- ---------------------------------------------------------------------------
-- SELECT indexrelname, pg_size_pretty(pg_relation_size(indexrelid)) AS size
-- FROM pg_stat_user_indexes
-- WHERE indexrelname LIKE 'ix_ovis_%'
-- ORDER BY 1;
--
-- A CONCURRENTLY build that is interrupted leaves an INVALID index behind.
-- Find and drop any:
-- SELECT c.relname FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
-- WHERE NOT i.indisvalid AND c.relname LIKE 'ix_ovis_%';

-- ---------------------------------------------------------------------------
-- Rollback
-- ---------------------------------------------------------------------------
-- DROP INDEX CONCURRENTLY IF EXISTS public.ix_ovis_document_updated;
-- DROP INDEX CONCURRENTLY IF EXISTS public.ix_ovis_document_chunk_count;
-- DROP INDEX CONCURRENTLY IF EXISTS public.ix_ovis_document_chunk_count_desc;
-- DROP INDEX CONCURRENTLY IF EXISTS public.ix_ovis_document_boost;
-- DROP INDEX CONCURRENTLY IF EXISTS public.ix_ovis_document_semantic_id_trgm;
-- DROP INDEX CONCURRENTLY IF EXISTS public.ix_ovis_document_id_trgm;
-- DROP INDEX CONCURRENTLY IF EXISTS public.ix_ovis_dcc_by_doc;
-- DROP INDEX CONCURRENTLY IF EXISTS public.ix_ovis_document_tag_by_tag;
-- (pg_trgm is left installed; it is harmless and may be in use elsewhere.)
