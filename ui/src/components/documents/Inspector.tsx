import { useEffect, useMemo, useState } from 'react';
import { useNavigate, useParams, useSearch, Link } from '@tanstack/react-router';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import {
  ChevronLeft,
  ChevronRight,
  Copy,
  ExternalLink,
  Pencil,
  Trash2,
  X,
} from 'lucide-react';
import { toast } from 'sonner';
import { pageDetailQuery, pagesInfiniteQuery } from '@/api/queries';
import type { PageDetail } from '@/api/types';
import { cn } from '@/lib/cn';
import { absolute, count as formatCount, relative, sourceLabel } from '@/lib/format';
import { pushRecentDoc } from '@/lib/recentDocs';
import { Badge, statusTone } from '@/components/primitives/Badge';
import { Button, IconButton } from '@/components/primitives/Button';
import { ErrorState } from '@/components/primitives/EmptyState';
import { Sheet } from '@/components/primitives/Sheet';
import { Skeleton } from '@/components/primitives/Skeleton';
import { TabsRoot, TabsList, TabsTrigger, TabsContent } from '@/components/primitives/Tabs';
import { useHotkeys } from '@/hooks/hotkeys';
import type { PagesSearch } from '@/routes/pages';
import { ChunksTab } from './inspector/ChunksTab';
import { DeleteDialog } from './inspector/DeleteDialog';
import { EditSheet } from './inspector/EditSheet';
import { JsonTree } from './inspector/JsonTree';
import { TextTab } from './inspector/TextTab';

function MetaRow({ label, value, mono }: { label: string; value: React.ReactNode; mono?: boolean }) {
  return (
    <div className="flex flex-col gap-0.5 py-1.5">
      <dt className="text-caption text-ink-faint">{label}</dt>
      <dd className={cn('text-body break-all text-ink-mute', mono && 'font-mono text-mono-sm')}>
        {value}
      </dd>
    </div>
  );
}

function OverviewTab({ detail }: { detail: PageDetail }) {
  return (
    <div className="space-y-4">
      <dl className="grid grid-cols-1 gap-x-6 sm:grid-cols-2">
        <MetaRow label="Updated (effective)" value={`${absolute(detail.updated_at)} · ${relative(detail.updated_at)}`} />
        <MetaRow
          label="Crawl-reported timestamp"
          value={
            detail.doc_updated_at ? (
              absolute(detail.doc_updated_at)
            ) : (
              <span className="text-ink-faint">not reported by the source</span>
            )
          }
        />
        <MetaRow label="Row touched" value={absolute(detail.last_modified)} />
        <MetaRow
          label="Last synced"
          value={detail.last_synced ? absolute(detail.last_synced) : <span className="text-ink-faint">—</span>}
        />
        <MetaRow
          label="Chunks"
          value={
            detail.chunk_count === null ? (
              <span className="text-ink-faint">not counted by Onyx yet — this is not zero</span>
            ) : (
              formatCount(detail.chunk_count)
            )
          }
        />
        <MetaRow label="Boost" value={detail.boost > 0 ? `+${detail.boost}` : detail.boost} />
        <MetaRow
          label="Content hash"
          mono
          value={detail.content_hash ?? <span className="text-ink-faint">—</span>}
        />
        <MetaRow
          label="Ingestion API"
          value={detail.from_ingestion_api ? 'yes' : 'no'}
        />
        {detail.primary_owners && detail.primary_owners.length > 0 ? (
          <MetaRow label="Primary owners" value={detail.primary_owners.join(', ')} />
        ) : null}
        {detail.secondary_owners && detail.secondary_owners.length > 0 ? (
          <MetaRow label="Secondary owners" value={detail.secondary_owners.join(', ')} />
        ) : null}
      </dl>

      {detail.tags.length > 0 ? (
        <div>
          <h3 className="mb-1.5 text-label text-ink-faint">Tags</h3>
          <div className="flex flex-wrap gap-1.5">
            {detail.tags.map((t, i) => (
              <Badge key={i} tone="teal">
                {t.key}: {t.value}
              </Badge>
            ))}
          </div>
        </div>
      ) : null}

      {detail.metadata && Object.keys(detail.metadata).length > 0 ? (
        <div>
          <h3 className="mb-1.5 text-label text-ink-faint">Metadata</h3>
          <JsonTree data={detail.metadata} copyLabel="Metadata" />
        </div>
      ) : null}
    </div>
  );
}

function InspectorBody({ docId, onClose }: { docId: string; onClose: () => void }) {
  const detail = useQuery(pageDetailQuery(docId));
  const search = useSearch({ strict: false }) as PagesSearch;
  const navigate = useNavigate();
  const [editOpen, setEditOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);

  useEffect(() => {
    if (detail.data) pushRecentDoc({ id: detail.data.id, title: detail.data.semantic_id });
  }, [detail.data]);

  // Prev/next over the background list (cache only — never a fresh fetch).
  const listCache = useInfiniteQuery({ ...pagesInfiniteQuery(search), enabled: false });
  const ids = useMemo(
    () => (listCache.data?.pages ?? []).flatMap((p) => p.items.map((i) => i.id)),
    [listCache.data],
  );
  const index = ids.indexOf(docId);
  const goTo = (id: string | undefined) => {
    if (!id) return;
    void navigate({
      to: '/pages/$docId',
      params: { docId: id },
      replace: true,
      search: (prev) => prev as PagesSearch,
    });
  };
  const prevId = index > 0 ? ids[index - 1] : undefined;
  const nextId = index >= 0 && index < ids.length - 1 ? ids[index + 1] : undefined;

  useHotkeys([
    {
      keys: '[',
      description: 'Previous document',
      group: 'Inspector',
      scope: 'sheet',
      handler: () => goTo(prevId),
    },
    {
      keys: ']',
      description: 'Next document',
      group: 'Inspector',
      scope: 'sheet',
      handler: () => goTo(nextId),
    },
    {
      keys: 'd',
      description: 'Delete document',
      group: 'Inspector',
      scope: 'sheet',
      handler: () => setDeleteOpen(true),
    },
    {
      keys: 'o',
      description: 'Open link in new tab',
      group: 'Inspector',
      scope: 'sheet',
      handler: () => {
        const link = detail.data?.link;
        if (link) window.open(link, '_blank', 'noopener');
      },
    },
  ]);

  if (detail.isPending) {
    return (
      <div className="space-y-3 p-5" aria-hidden>
        <Skeleton className="h-6 w-2/3" />
        <Skeleton className="h-4 w-full" />
        <Skeleton className="h-4 w-1/2" />
        <Skeleton className="mt-6 h-40 w-full rounded-xl" />
      </div>
    );
  }
  if (detail.isError) {
    return (
      <ErrorState
        error={detail.error}
        title="Document could not load"
        onRetry={() => void detail.refetch()}
      />
    );
  }

  const d = detail.data;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="shrink-0 space-y-2.5 border-b border-line px-5 pt-4 pb-3">
        <div className="flex items-start justify-between gap-2">
          <div className="flex flex-wrap items-center gap-1.5">
            {d.connector_name && d.cc_pair_id !== null ? (
              <Link to="/connectors/$ccPairId" params={{ ccPairId: d.cc_pair_id }}>
                <Badge tone="violet" className="hover:bg-violet/25">
                  {d.connector_name}
                  {d.connector_source ? ` · ${sourceLabel(d.connector_source)}` : ''}
                </Badge>
              </Link>
            ) : d.connector_name ? (
              <Badge tone="violet">{d.connector_name}</Badge>
            ) : null}
            {d.cc_pair_status ? <Badge tone={statusTone(d.cc_pair_status)}>{d.cc_pair_status}</Badge> : null}
            {d.hidden ? <Badge tone="neutral">hidden</Badge> : null}
            {d.recrawl_risk ? (
              <Badge tone="gold" title="The owning cc-pair is active — deletes are liable to be undone by the next refresh">
                recrawl risk
              </Badge>
            ) : null}
          </div>
          <div className="flex shrink-0 items-center gap-0.5">
            <IconButton label="Previous document ([)" disabled={!prevId} onClick={() => goTo(prevId)}>
              <ChevronLeft className="size-4" aria-hidden />
            </IconButton>
            <IconButton label="Next document (])" disabled={!nextId} onClick={() => goTo(nextId)}>
              <ChevronRight className="size-4" aria-hidden />
            </IconButton>
            <IconButton label="Close" onClick={onClose}>
              <X className="size-4" aria-hidden />
            </IconButton>
          </div>
        </div>

        {!d.pg_row ? (
          <div className="rounded-lg border border-gold/30 bg-gold/10 px-3 py-2 text-label text-gold">
            Chunks exist but the Postgres row is gone — orphaned index entries, a cleanup candidate.
          </div>
        ) : null}

        <h2 className="font-display font-display-soft text-title text-ink select-text">
          {d.semantic_id}
        </h2>

        <div className="flex items-center gap-1.5">
          <span className="min-w-0 flex-1 truncate font-mono text-mono-sm text-ink-faint select-all">
            {d.link ?? d.id}
          </span>
          <IconButton
            label="Copy URL"
            onClick={() => {
              void navigator.clipboard.writeText(d.link ?? d.id);
              toast('URL copied');
            }}
          >
            <Copy className="size-4" aria-hidden />
          </IconButton>
          {d.link ? (
            <IconButton label="Open in new tab (o)" onClick={() => window.open(d.link!, '_blank', 'noopener')}>
              <ExternalLink className="size-4" aria-hidden />
            </IconButton>
          ) : null}
        </div>
      </header>

      <TabsRoot defaultValue="overview" className="flex min-h-0 flex-1 flex-col">
        <TabsList className="shrink-0 px-5">
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="text">Text</TabsTrigger>
          <TabsTrigger value="chunks">Chunks</TabsTrigger>
          <TabsTrigger value="json">JSON</TabsTrigger>
        </TabsList>
        <TabsContent value="overview" className="min-h-0 flex-1 overflow-y-auto p-5">
          <OverviewTab detail={d} />
        </TabsContent>
        <TabsContent value="text" className="min-h-0 flex-1 overflow-hidden p-5">
          <TextTab docId={docId} />
        </TabsContent>
        <TabsContent value="chunks" className="min-h-0 flex-1 overflow-y-auto p-5">
          <ChunksTab docId={docId} />
        </TabsContent>
        <TabsContent value="json" className="min-h-0 flex-1 overflow-y-auto p-5">
          <JsonTree data={d} copyLabel="Document JSON" />
        </TabsContent>
      </TabsRoot>

      <footer
        className="flex shrink-0 items-center justify-end gap-2 border-t border-line px-5 py-3"
        style={{ paddingBottom: 'max(env(safe-area-inset-bottom), 0.75rem)' }}
      >
        <Button variant="secondary" onClick={() => setEditOpen(true)}>
          <Pencil className="size-4" aria-hidden /> Edit
        </Button>
        <Button variant="destructive" onClick={() => setDeleteOpen(true)}>
          <Trash2 className="size-4" aria-hidden /> Delete
        </Button>
      </footer>

      {editOpen ? <EditSheet key={d.id} detail={d} open={editOpen} onOpenChange={setEditOpen} /> : null}
      <DeleteDialog
        detail={d}
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        onDeleted={onClose}
      />
    </div>
  );
}

/**
 * `/pages/$docId` — a Sheet over the list (right panel on desktop, bottom
 * sheet on mobile). The docId is one percent-encoded path segment; the param
 * arrives decoded and round-trips ids containing `/`, `?` and `#`.
 */
export function InspectorRoute() {
  const { docId } = useParams({ from: '/pages/$docId' });
  const navigate = useNavigate();
  const [open, setOpen] = useState(true);

  const close = () => {
    setOpen(false);
    // Let the sheet play its exit before the route unmounts it.
    setTimeout(() => {
      void navigate({ to: '/pages', search: (prev) => prev as PagesSearch });
    }, 220);
  };

  return (
    <Sheet
      open={open}
      onOpenChange={(o) => {
        if (!o) close();
      }}
      title="Document inspector"
    >
      <InspectorBody docId={docId} onClose={close} />
    </Sheet>
  );
}
