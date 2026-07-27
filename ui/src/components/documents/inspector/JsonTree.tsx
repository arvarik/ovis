import { toast } from 'sonner';
import { Copy } from 'lucide-react';
import { Button } from '@/components/primitives/Button';

/**
 * The promised-but-never-built collapsible JSON tree: plain `<details>`
 * nodes — natively keyboard-focusable and screen-reader friendly.
 */
function Node({ name, value, depth }: { name: string | null; value: unknown; depth: number }) {
  const label = name !== null ? <span className="text-teal">{name}</span> : null;

  if (value === null)
    return (
      <div className="py-0.5">
        {label}
        {label ? ': ' : ''}
        <span className="text-ink-faint italic">null</span>
      </div>
    );
  if (typeof value === 'string')
    return (
      <div className="py-0.5 break-all">
        {label}
        {label ? ': ' : ''}
        <span className="text-mint">“{value}”</span>
      </div>
    );
  if (typeof value === 'number' || typeof value === 'boolean')
    return (
      <div className="py-0.5">
        {label}
        {label ? ': ' : ''}
        <span className="text-gold">{String(value)}</span>
      </div>
    );

  const entries = Array.isArray(value)
    ? value.map((v, i) => [String(i), v] as const)
    : Object.entries(value as Record<string, unknown>);
  const preview = Array.isArray(value) ? `[${entries.length}]` : `{${entries.length}}`;

  if (entries.length === 0)
    return (
      <div className="py-0.5">
        {label}
        {label ? ': ' : ''}
        <span className="text-ink-faint">{Array.isArray(value) ? '[]' : '{}'}</span>
      </div>
    );

  return (
    <details open={depth < 2} className="group">
      <summary className="cursor-pointer rounded py-0.5 select-none focus-visible:outline-2 [&::marker]:text-ink-faint">
        {label}
        {label ? ': ' : ''}
        <span className="text-ink-faint">{preview}</span>
      </summary>
      <div className="border-l border-line pl-4">
        {entries.map(([k, v]) => (
          <Node key={k} name={k} value={v} depth={depth + 1} />
        ))}
      </div>
    </details>
  );
}

export function JsonTree({ data, copyLabel }: { data: unknown; copyLabel: string }) {
  return (
    <div className="space-y-3">
      <Button
        variant="secondary"
        size="sm"
        onClick={() => {
          void navigator.clipboard.writeText(JSON.stringify(data, null, 2));
          toast(`${copyLabel} copied`);
        }}
      >
        <Copy className="size-4" aria-hidden /> Copy JSON
      </Button>
      <div className="rounded-lg border border-line bg-well p-3 font-mono text-mono-sm text-ink">
        <Node name={null} value={data} depth={0} />
      </div>
    </div>
  );
}
