-- Prune-track fixtures, applied ON TOP of seed.sql by the prune integration
-- tests only (the base counts that api_contract.rs asserts stay untouched).
--
-- Shapes covered:
--   * an exact-duplicate group (same content_hash, three URL lengths) on the
--     PAUSED connector — keeper policy is observable
--   * aged stubs on both a PAUSED pair (durable delete) and the ACTIVE pair
--     (recrawl_risk must be true)
--   * a stub that was already hidden before pruning — prev_hidden must
--     round-trip exactly
--   * a German-language page and a near-duplicate pair whose *content* lives
--     in the OpenSearch stand-in (rows here carry distinct hashes)

BEGIN;

INSERT INTO public.document
    (id, boost, hidden, semantic_id, link, doc_updated_at, last_modified, last_synced,
     chunk_count, doc_metadata, from_ingestion_api, content_hash)
VALUES
    -- Exact-duplicate group. Shortest URL is the canonical one.
    ('https://paused.example/dup', 0, false, 'Dup Canonical',
     'https://paused.example/dup',
     NULL, now() - interval '20 days', NULL, 4, '{}'::jsonb, false, 'prune-dup-hash-1'),
    ('https://paused.example/dup?utm_source=feed', 0, false, 'Dup With Tracking',
     'https://paused.example/dup?utm_source=feed',
     NULL, now() - interval '21 days', NULL, 4, '{}'::jsonb, false, 'prune-dup-hash-1'),
    ('https://paused.example/dup/print/view', 0, false, 'Dup Print View',
     'https://paused.example/dup/print/view',
     NULL, now() - interval '22 days', NULL, 3, '{}'::jsonb, false, 'prune-dup-hash-1'),
    -- Aged stub on the PAUSED pair: flaggable, and deleting it is durable.
    ('https://paused.example/old-stub', 0, false, 'Old Stub',
     'https://paused.example/old-stub',
     NULL, now() - interval '30 days', NULL, 0, '{}'::jsonb, false, NULL),
    -- Aged stub on the ACTIVE pair: flaggable, recrawl_risk = true.
    ('https://example.com/active-stub', 0, false, 'Active Stub',
     'https://example.com/active-stub',
     NULL, now() - interval '40 days', NULL, 0, '{}'::jsonb, false, NULL),
    -- Already hidden before pruning; restore must return it to hidden=true.
    ('https://paused.example/already-hidden-stub', 0, true, 'Hidden Stub',
     'https://paused.example/already-hidden-stub',
     NULL, now() - interval '25 days', NULL, 0, '{}'::jsonb, false, NULL),
    -- German-language page; its chunk text is served by the OpenSearch mock.
    ('https://paused.example/de/impressum', 0, false, 'Impressum',
     'https://paused.example/de/impressum',
     NULL, now() - interval '15 days', NULL, 2, '{}'::jsonb, false, NULL),
    -- Near-duplicate pair: different hashes, ~identical chunk text in the mock.
    ('https://paused.example/guide', 0, false, 'Guide',
     'https://paused.example/guide',
     NULL, now() - interval '12 days', NULL, 2, '{}'::jsonb, false, 'prune-near-a'),
    ('https://paused.example/guide-copy', 0, false, 'Guide (copy)',
     'https://paused.example/guide-copy',
     NULL, now() - interval '11 days', NULL, 2, '{}'::jsonb, false, 'prune-near-b');

INSERT INTO public.document_by_connector_credential_pair (id, connector_id, credential_id, has_been_indexed)
VALUES
    ('https://paused.example/dup', 2, 1, true),
    ('https://paused.example/dup?utm_source=feed', 2, 1, true),
    ('https://paused.example/dup/print/view', 2, 1, true),
    ('https://paused.example/old-stub', 2, 1, true),
    ('https://example.com/active-stub', 1, 1, true),
    ('https://paused.example/already-hidden-stub', 2, 1, true),
    ('https://paused.example/de/impressum', 2, 1, true),
    ('https://paused.example/guide', 2, 1, true),
    ('https://paused.example/guide-copy', 2, 1, true);

-- A tag on one duplicate, so the reaper's cascade has an FK child to clear.
INSERT INTO public.document__tag (document_id, tag_id)
VALUES ('https://paused.example/dup/print/view', 3);

COMMIT;
