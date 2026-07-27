import { useMemo, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useVirtualizer } from '@tanstack/react-virtual';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Copy, Download, Search } from 'lucide-react';
import { toast } from 'sonner';
import { pageTextQuery } from '@/api/queries';
import { encodeDocId } from '@/api/client';
import { cn } from '@/lib/cn';
import { Button } from '@/components/primitives/Button';
import { ErrorState } from '@/components/primitives/EmptyState';
import { Skeleton } from '@/components/primitives/Skeleton';

/** Markdown-ish? Headings, fences, emphasis or links — else plain text. */
function looksLikeMarkdown(text: string): boolean {
  return /^#{1,6}\s.+|^```|\*\*[^*\n]+\*\*|\[[^\]\n]+\]\([^)\n]+\)/m.test(text.slice(0, 4000));
}

const MD_COMPONENTS = {
  h1: (p: React.ComponentProps<'h1'>) => (
    <h1 className="mt-5 mb-2 font-display font-display-soft text-display text-ink" {...p} />
  ),
  h2: (p: React.ComponentProps<'h2'>) => (
    <h2 className="mt-4 mb-2 font-display font-display-soft text-title text-ink" {...p} />
  ),
  h3: (p: React.ComponentProps<'h3'>) => (
    <h3 className="mt-3 mb-1.5 text-body font-semibold text-ink" {...p} />
  ),
  p: (p: React.ComponentProps<'p'>) => <p className="my-2 text-body text-ink-mute" {...p} />,
  a: (p: React.ComponentProps<'a'>) => (
    <a className="text-teal underline decoration-teal/40 underline-offset-2" target="_blank" rel="noopener noreferrer" {...p} />
  ),
  li: (p: React.ComponentProps<'li'>) => <li className="my-1 ml-5 list-disc text-body text-ink-mute" {...p} />,
  code: (p: React.ComponentProps<'code'>) => (
    <code className="rounded bg-well px-1 py-0.5 font-mono text-mono-sm text-mint" {...p} />
  ),
  pre: (p: React.ComponentProps<'pre'>) => (
    <pre className="my-3 overflow-x-auto rounded-lg border border-line bg-well p-3 font-mono text-mono-sm [&_code]:bg-transparent [&_code]:p-0" {...p} />
  ),
  blockquote: (p: React.ComponentProps<'blockquote'>) => (
    <blockquote className="my-2 border-l-2 border-line-3 pl-3 text-ink-faint" {...p} />
  ),
  table: (p: React.ComponentProps<'table'>) => (
    <div className="my-3 overflow-x-auto">
      <table className="w-full text-label [&_td]:border [&_td]:border-line [&_td]:px-2 [&_td]:py-1 [&_th]:border [&_th]:border-line [&_th]:bg-surface [&_th]:px-2 [&_th]:py-1" {...p} />
    </div>
  ),
};

export function TextTab({ docId }: { docId: string }) {
  const text = useQuery(pageTextQuery(docId));
  const [find, setFind] = useState('');
  const scrollRef = useRef<HTMLDivElement>(null);

  const lines = useMemo(() => (text.data ?? '').split('\n'), [text.data]);
  const isMarkdown = useMemo(() => (text.data ? looksLikeMarkdown(text.data) : false), [text.data]);

  const matches = useMemo(() => {
    if (!find.trim()) return [];
    const needle = find.toLowerCase();
    const out: number[] = [];
    lines.forEach((line, i) => {
      if (line.toLowerCase().includes(needle)) out.push(i);
    });
    return out;
  }, [find, lines]);

  const virtualizer = useVirtualizer({
    count: lines.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 22,
    overscan: 30,
  });

  if (text.isPending)
    return (
      <div className="space-y-2 p-1" aria-hidden>
        {Array.from({ length: 10 }, (_, i) => (
          <Skeleton key={i} className={cn('h-4', i % 3 === 0 ? 'w-4/5' : i % 3 === 1 ? 'w-full' : 'w-2/3')} />
        ))}
      </div>
    );
  if (text.isError)
    return <ErrorState error={text.error} title="Text could not load" onRetry={() => void text.refetch()} />;

  const data = text.data;

  return (
    <div className="flex h-full min-h-0 flex-col gap-3">
      <div className="flex shrink-0 flex-wrap items-center gap-2">
        <div className="flex min-w-40 flex-1 items-center gap-2 rounded-lg border border-line bg-well px-2.5">
          <Search className="size-3.5 shrink-0 text-ink-faint" aria-hidden />
          <input
            value={find}
            onChange={(e) => {
              setFind(e.target.value);
              const needle = e.target.value.toLowerCase();
              const first = needle ? lines.findIndex((l) => l.toLowerCase().includes(needle)) : -1;
              if (first >= 0 && !isMarkdown) virtualizer.scrollToIndex(first, { align: 'center' });
            }}
            placeholder="Find in text…"
            aria-label="Find in text"
            className="min-h-9 w-full bg-transparent text-body text-ink outline-none placeholder:text-ink-faint"
          />
          {find ? (
            <span className="shrink-0 font-mono text-caption text-ink-faint">
              {matches.length} line{matches.length === 1 ? '' : 's'}
            </span>
          ) : null}
        </div>
        <Button
          variant="secondary"
          size="sm"
          onClick={() => {
            void navigator.clipboard.writeText(data);
            toast('Text copied');
          }}
        >
          <Copy className="size-4" aria-hidden /> Copy
        </Button>
        <a
          href={`/api/v1/pages/${encodeDocId(docId)}/text?download=1`}
          download
          className="inline-flex min-h-11 items-center justify-center gap-2 rounded-lg border border-line-2 bg-surface px-3 text-label text-ink transition-colors hover:bg-hover md:min-h-8"
        >
          <Download className="size-4" aria-hidden /> Download
        </a>
      </div>

      {isMarkdown ? (
        <div className="min-h-0 flex-1 overflow-y-auto pr-1">
          <Markdown remarkPlugins={[remarkGfm]} components={MD_COMPONENTS}>
            {data}
          </Markdown>
        </div>
      ) : (
        <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto rounded-lg border border-line bg-well">
          <div className="relative w-full font-mono text-mono-sm" style={{ height: virtualizer.getTotalSize() }}>
            {virtualizer.getVirtualItems().map((vi) => {
              const line = lines[vi.index] ?? '';
              const hit = find.trim() !== '' && line.toLowerCase().includes(find.toLowerCase());
              return (
                <div
                  key={vi.key}
                  className={cn('absolute inset-x-0 flex gap-3 px-3 whitespace-pre', hit && 'bg-gold/10')}
                  style={{ top: 0, transform: `translateY(${vi.start}px)`, height: vi.size }}
                >
                  <span className="w-10 shrink-0 text-right text-ink-faint select-none">{vi.index + 1}</span>
                  <span className={cn('text-ink-mute', hit && 'text-ink')}>{line}</span>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
