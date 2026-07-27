-- Seed data for the OVIS integration tests.
--
-- Deliberately shaped around the things that broke before, not around a happy
-- path:
--
--   * a document with tags and chunk_stats rows, so a cascading delete has real
--     foreign-key children to clear (the old delete cleared none of these and
--     failed on any tagged document — and 444,793 tag links exist in production)
--   * documents belonging to two connectors, so counts cannot double-count
--   * a NULL `chunk_count`, which is not the same as zero
--   * a NULL `doc_updated_at`, which is the case for all but ~1,500 of the
--     1.65M production rows, so ordering must fall back to `last_modified`
--   * timestamps that are deliberately *not* in id order, so a lexicographic
--     sort is distinguishable from a recency sort
--   * a PAUSED connector, an ACTIVE one, and one parked by the resilience cron
--   * an `index_attempt` that is IN_PROGRESS and stale (stalled) and one that is
--     IN_PROGRESS and fresh (not stalled)

BEGIN;

-- ---------------------------------------------------------------------------
-- search_settings: two rows, only one PRESENT. Resolving the index name from
-- the wrong row is exactly the `danswer_chunk*` wildcard bug.
-- ---------------------------------------------------------------------------
INSERT INTO public.search_settings
    (id, model_name, model_dim, normalize, query_prefix, passage_prefix, index_name,
     status, multipass_indexing, embedding_precision, enable_contextual_rag, switchover_type)
VALUES
    (1, 'thenlper/gte-small', 384, true, '', '', 'danswer_chunk', 'PAST', false, 'FLOAT', false, 'REINDEX'),
    (2, 'snowflake-arctic-embed:m', 768, true, '', '', 'danswer_chunk_snowflake_arctic_embed_m',
     'PRESENT', false, 'FLOAT', false, 'REINDEX');

-- ---------------------------------------------------------------------------
-- credentials and connectors
-- ---------------------------------------------------------------------------
INSERT INTO public.credential (id, admin_public, source, name, curator_public, time_created, time_updated)
VALUES
    (1, true, 'WEB', 'web-credential', true, now(), now()),
    (2, true, 'GITHUB', 'github-credential', true, now(), now());

INSERT INTO public.connector
    (id, name, source, input_type, connector_specific_config, refresh_freq, prune_freq,
     time_created, time_updated)
VALUES
    (1, 'tildes-like', 'WEB', 'load_state',
     '{"base_url": "https://example.com/", "web_connector_type": "recursive"}'::jsonb,
     2592000, NULL, now(), now()),
    (2, 'paused-web', 'WEB', 'load_state',
     '{"base_url": "https://paused.example/"}'::jsonb, 2592000, NULL, now(), now()),
    (3, 'parked-web', 'WEB', 'load_state',
     '{"base_url": "https://parked.example/"}'::jsonb, 2592000, NULL, now(), now()),
    (4, 'code-mirror', 'GITHUB', 'poll',
     '{"repo_owner": "example", "repositories": "thing"}'::jsonb, 86400, NULL, now(), now());

INSERT INTO public.connector_credential_pair
    (id, connector_id, credential_id, name, status, access_type, total_docs_indexed,
     indexing_trigger, in_repeated_error_state, processing_mode, last_successful_index_time)
VALUES
    -- total_docs_indexed is deliberately wrong here (0 and 99999): it is
    -- unreliable in production and must never be read.
    (1, 1, 1, 'tildes-like', 'ACTIVE',  'PUBLIC', 0,     NULL,     false, 'STANDARD', now() - interval '1 hour'),
    (2, 2, 1, 'paused-web',  'PAUSED',  'PUBLIC', 99999, NULL,     false, 'STANDARD', now() - interval '9 days'),
    (3, 3, 1, 'parked-web',  'PAUSED',  'PUBLIC', 0,     NULL,     true,  'STANDARD', NULL),
    (4, 4, 2, 'code-mirror', 'INITIAL_INDEXING', 'PUBLIC', 0, 'UPDATE', false, 'STANDARD', NULL);

-- ---------------------------------------------------------------------------
-- documents
--
-- id order and recency order disagree on purpose: 'https://example.com/aaa' is
-- alphabetically first but the *oldest*. A listing that returns it first is
-- sorting by URL, which is the bug this fixture exists to catch.
-- ---------------------------------------------------------------------------
INSERT INTO public.document
    (id, boost, hidden, semantic_id, link, doc_updated_at, last_modified, last_synced,
     chunk_count, doc_metadata, from_ingestion_api)
VALUES
    ('https://example.com/aaa', 0, false, 'Oldest Page', 'https://example.com/aaa',
     NULL, now() - interval '30 days', now() - interval '30 days', 4,
     '{"author": "alice", "keep": "me"}'::jsonb, false),
    ('https://example.com/bbb', 0, false, 'Middle Page', 'https://example.com/bbb',
     NULL, now() - interval '10 days', now() - interval '10 days', 12,
     '{"author": "bob"}'::jsonb, false),
    ('https://example.com/ccc', 3, false, 'Newest Page', 'https://example.com/ccc',
     now() - interval '1 hour', now() - interval '2 days', now() - interval '2 days', 7,
     '{"author": "alice"}'::jsonb, false),
    -- A stub: zero chunks. The "stubs" preset is chunk_min=0&chunk_max=0.
    ('https://example.com/stub', 0, false, 'Stub Page', 'https://example.com/stub',
     NULL, now() - interval '5 days', NULL, 0, '{}'::jsonb, false),
    -- chunk_count NULL: Onyx has not counted this one. Not the same as zero, and
    -- excluded from both chunk bounds.
    ('https://example.com/uncounted', 0, false, 'Uncounted Page', NULL,
     NULL, now() - interval '3 days', NULL, NULL, NULL, false),
    -- Hidden from search.
    ('https://example.com/hidden', 0, true, 'Hidden Page', 'https://example.com/hidden',
     NULL, now() - interval '4 days', NULL, 2, '{}'::jsonb, false),
    -- Belongs to two connectors: must be counted once.
    ('https://example.com/shared', 0, false, 'Shared Page', 'https://example.com/shared',
     NULL, now() - interval '6 days', NULL, 5, '{}'::jsonb, false),
    -- GITHUB source, for source-filter tests.
    ('https://github.com/example/thing/blob/main/README.md', 0, false, 'README',
     'https://github.com/example/thing/blob/main/README.md',
     NULL, now() - interval '7 days', NULL, 3, '{}'::jsonb, false),
    -- The delete target: has tags, chunk_stats and retrieval feedback hanging off
    -- it, and an ACTIVE connector, so its delete reports recrawl_risk.
    ('https://example.com/deleteme', 0, false, 'Delete Me', 'https://example.com/deleteme',
     NULL, now() - interval '8 days', NULL, 6, '{"author": "carol"}'::jsonb, false),
    -- Percent-encoding torture: query string, ampersand, space, unicode.
    ('https://example.com/tricky?a=1&b=2 c=café', 0, false, 'Tricky Id',
     'https://example.com/tricky?a=1&b=2 c=café',
     NULL, now() - interval '9 days', NULL, 1, '{}'::jsonb, false);

INSERT INTO public.document_by_connector_credential_pair (id, connector_id, credential_id, has_been_indexed)
VALUES
    ('https://example.com/aaa', 1, 1, true),
    ('https://example.com/bbb', 1, 1, true),
    ('https://example.com/ccc', 1, 1, true),
    ('https://example.com/stub', 1, 1, true),
    ('https://example.com/uncounted', 1, 1, false),
    ('https://example.com/hidden', 1, 1, true),
    -- Two connectors for one document.
    ('https://example.com/shared', 1, 1, true),
    ('https://example.com/shared', 2, 1, true),
    ('https://github.com/example/thing/blob/main/README.md', 4, 2, true),
    ('https://example.com/deleteme', 1, 1, true),
    ('https://example.com/tricky?a=1&b=2 c=café', 2, 1, true);

-- ---------------------------------------------------------------------------
-- Foreign-key children of `document`. These are what the old delete forgot.
-- ---------------------------------------------------------------------------
INSERT INTO public.tag (id, tag_key, tag_value, source, is_list)
VALUES
    (1, 'author', 'carol', 'WEB', false),
    (2, 'author', 'alice', 'WEB', false),
    (3, 'topic', 'economics', 'WEB', false);

INSERT INTO public.document__tag (document_id, tag_id)
VALUES
    ('https://example.com/deleteme', 1),
    ('https://example.com/deleteme', 3),
    ('https://example.com/aaa', 2),
    ('https://example.com/ccc', 2);

INSERT INTO public.chunk_stats
    (id, document_id, chunk_in_doc_id, information_content_boost, last_modified)
VALUES
    ('https://example.com/deleteme__0', 'https://example.com/deleteme', 0, 1.0, now()),
    ('https://example.com/deleteme__1', 'https://example.com/deleteme', 1, 1.0, now());

-- ---------------------------------------------------------------------------
-- index_attempt telemetry
-- ---------------------------------------------------------------------------
INSERT INTO public.index_attempt
    (id, connector_credential_pair_id, search_settings_id, status, error_msg,
     new_docs_indexed, total_docs_indexed, total_chunks, completed_batches, total_batches,
     total_failures_batch_level, from_beginning, cancellation_requested,
     last_batches_completed_count, heartbeat_counter, last_heartbeat_value, last_heartbeat_time,
     time_created, time_started, time_updated)
VALUES
    -- Fresh IN_PROGRESS: must NOT be flagged stalled, even at zero new docs.
    (1, 1, 2, 'IN_PROGRESS', NULL, 0, 0, 0, 1, 10, 0, false, false, 0, 5, 5,
     now() - interval '1 minute', now() - interval '2 hours', now() - interval '90 minutes',
     now() - interval '1 minute'),
    -- Stale IN_PROGRESS: no heartbeat for hours ⇒ stalled.
    (2, 4, 2, 'IN_PROGRESS', NULL, 120, 240, 900, 3, 8, 0, false, false, 0, 2, 2,
     now() - interval '3 hours', now() - interval '4 hours', now() - interval '4 hours',
     now() - interval '3 hours'),
    (3, 1, 2, 'SUCCESS', NULL, 50, 50, 200, 5, 5, 0, false, false, 0, 9, 9,
     now() - interval '1 day', now() - interval '1 day', now() - interval '1 day',
     now() - interval '1 day'),
    (4, 2, 2, 'FAILED', 'connection reset by peer', 0, 0, 0, 0, 3, 1, false, false, 0, 0, 0,
     NULL, now() - interval '2 days', now() - interval '2 days', now() - interval '2 days'),
    -- Park sentinel: the resilience cron wrote this and OVIS must never clobber
    -- it. cc-pair 3 shows up as `parked` because of this row.
    (5, 3, 2, 'FAILED', 'first-pass already complete', 0, 0, 0, 0, 0, 0, false, false, 0, 0, 0,
     NULL, now() - interval '3 days', now() - interval '3 days', now() - interval '3 days'),
    (6, 1, 2, 'CANCELED', NULL, 0, 0, 0, 0, 0, 0, true, true, 0, 0, 0,
     NULL, now() - interval '4 days', now() - interval '4 days', now() - interval '4 days');

INSERT INTO public.index_attempt_errors
    (id, index_attempt_id, connector_credential_pair_id, document_id, document_link,
     failure_message, is_resolved, time_created, error_type)
VALUES
    (1, 4, 2, 'https://paused.example/broken', 'https://paused.example/broken',
     'HTTP 403 from origin', false, now() - interval '2 hours', 'FETCH'),
    (2, 4, 2, 'https://paused.example/timeout', 'https://paused.example/timeout',
     'read timed out', true, now() - interval '3 hours', 'FETCH');

INSERT INTO public.background_error (id, message, time_created, cc_pair_id)
VALUES (1, 'celery worker lost heartbeat', now() - interval '30 minutes', 2);

-- Keep sequences ahead of the hand-assigned ids so later inserts do not collide.
-- Conditional because not every Onyx id column is sequence-backed (search_settings
-- is not), and a hard setval on a missing sequence would fail the whole seed.
DO $$
DECLARE seq text;
BEGIN
    FOREACH seq IN ARRAY ARRAY[
        'connector_id_seq',
        'connector_credential_pair_id_seq',
        'credential_id_seq',
        'index_attempt_id_seq',
        'index_attempt_errors_id_seq',
        'tag_id_seq',
        'search_settings_id_seq',
        'background_error_id_seq'
    ] LOOP
        IF EXISTS (
            SELECT 1 FROM pg_class c
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE c.relkind = 'S' AND n.nspname = 'public' AND c.relname = seq
        ) THEN
            PERFORM setval('public.' || seq, 100, true);
        END IF;
    END LOOP;
END $$;

COMMIT;
