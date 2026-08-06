/**
 * Rules: URL/tag patterns and detector configuration. Every rule starts
 * disabled, and the enable switch waits until a preview has been run at
 * least once this session — the API enforces nothing here; the UI simply
 * orders the flow (04 §1).
 */
import { useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { FileDown, FileUp, ListChecks } from 'lucide-react';
import { pruneConfigQuery, pruneRulesQuery, tagKeysQuery } from '@/api/queries';
import {
  usePruneConfigImport,
  usePruneRuleCreate,
  usePruneRuleDelete,
  usePruneRulePatch,
  usePruneRulePreview,
} from '@/api/mutations';
import type { PruneRuleItem, PruneRulePreviewResponse } from '@/api/types';
import { Badge } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { Dialog } from '@/components/primitives/Dialog';
import { EmptyState, ErrorState } from '@/components/primitives/EmptyState';
import { Input } from '@/components/primitives/Input';
import { Select } from '@/components/primitives/Select';
import { Skeleton } from '@/components/primitives/Skeleton';
import { count as formatCount, relative } from '@/lib/format';
import { ExclusionsCard } from './ExclusionsCard';

export function RulesTab() {
  const rules = useQuery(pruneRulesQuery);
  const [previewedIds, setPreviewedIds] = useState<ReadonlySet<number>>(new Set());
  const [previewing, setPreviewing] = useState<PruneRuleItem | null>(null);
  const [creating, setCreating] = useState(false);
  const [showConfig, setShowConfig] = useState(false);
  const patch = usePruneRulePatch();
  const remove = usePruneRuleDelete();

  if (rules.isError) {
    return <ErrorState error={rules.error} onRetry={() => void rules.refetch()} />;
  }

  return (
    <div className="space-y-4">
      <Card className="space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h2 className="font-display font-display-soft text-title text-ink">Detection rules</h2>
          <div className="flex gap-2">
            <Button size="sm" onClick={() => setShowConfig(true)}>
              <FileDown className="size-4" aria-hidden /> Config YAML
            </Button>
            <Button size="sm" variant="primary" onClick={() => setCreating(true)}>
              New rule
            </Button>
          </div>
        </div>

        {rules.isPending ? (
          <div className="space-y-2">
            <Skeleton className="h-10" />
            <Skeleton className="h-10" />
          </div>
        ) : rules.data.length === 0 ? (
          <EmptyState
            icon={<ListChecks aria-hidden />}
            title="No rules yet"
            description="URL and tag rules flag documents by pattern. They start disabled; preview one against live data before enabling it."
          />
        ) : (
          <ul className="divide-y divide-line">
            {rules.data.map((rule) => {
              const pattern =
                typeof rule.body.pattern === 'string' ? rule.body.pattern : null;
              const confidence =
                typeof rule.body.confidence === 'number' ? rule.body.confidence : null;
              const canToggle = rule.enabled || previewedIds.has(rule.id) || rule.kind === 'detector_config';
              return (
                <li key={rule.id} className="flex flex-wrap items-center gap-2 py-2.5">
                  <div className="min-w-0 flex-1">
                    <p className="flex items-center gap-2 text-label text-ink">
                      {rule.name}
                      <Badge tone={rule.enabled ? 'mint' : 'neutral'}>
                        {rule.enabled ? 'enabled' : 'disabled'}
                      </Badge>
                      <Badge tone="neutral">{rule.kind.replace('_', ' ')}</Badge>
                    </p>
                    <p className="mt-0.5 truncate font-mono text-caption text-ink-faint">
                      {pattern ?? 'detector configuration'}
                      {confidence !== null ? `  ·  confidence ${confidence}` : ''}
                    </p>
                  </div>
                  <span className="text-caption text-ink-faint">{relative(rule.updated_at)}</span>
                  {rule.kind !== 'detector_config' ? (
                    <Button size="sm" onClick={() => setPreviewing(rule)}>
                      Preview
                    </Button>
                  ) : null}
                  <Button
                    size="sm"
                    variant={rule.enabled ? 'secondary' : 'primary'}
                    disabled={!canToggle || patch.isPending}
                    title={
                      canToggle
                        ? undefined
                        : 'Preview this rule against live data before enabling it'
                    }
                    onClick={() => patch.mutate({ id: rule.id, enabled: !rule.enabled })}
                  >
                    {rule.enabled ? 'Disable' : 'Enable'}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={remove.isPending}
                    onClick={() => remove.mutate(rule.id)}
                  >
                    Delete
                  </Button>
                </li>
              );
            })}
          </ul>
        )}
      </Card>

      {/* Same question as the rules above — "what will a scan flag?" — from the
          other side, which is where you look when a document is *not* being
          flagged and you want to know why. */}
      <ExclusionsCard />

      <PreviewDialog
        rule={previewing}
        onClose={() => setPreviewing(null)}
        onPreviewed={(id) => setPreviewedIds((prev) => new Set(prev).add(id))}
      />
      <CreateRuleDialog open={creating} onOpenChange={setCreating} />
      <ConfigDialog open={showConfig} onOpenChange={setShowConfig} />
    </div>
  );
}

function PreviewDialog({
  rule,
  onClose,
  onPreviewed,
}: {
  rule: PruneRuleItem | null;
  onClose: () => void;
  onPreviewed: (id: number) => void;
}) {
  const preview = usePruneRulePreview();
  const [result, setResult] = useState<PruneRulePreviewResponse | null>(null);
  const open = rule !== null;

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) {
          setResult(null);
          preview.reset();
          onClose();
        }
      }}
      title={rule ? `Preview '${rule.name}'` : 'Preview'}
      description="Runs the pattern against live data. Nothing is flagged or changed."
    >
      {rule ? (
        <div className="space-y-3">
          <p className="break-all font-mono text-caption text-ink-mute">
            {typeof rule.body.pattern === 'string' ? rule.body.pattern : ''}
          </p>
          <Button
            variant="primary"
            disabled={preview.isPending}
            onClick={() =>
              preview.mutate(rule.id, {
                onSuccess: (data) => {
                  setResult(data);
                  onPreviewed(rule.id);
                },
              })
            }
          >
            {preview.isPending ? 'Sampling…' : 'Run preview'}
          </Button>

          {result ? (
            <div className="space-y-2">
              <p className="text-label text-ink">
                {result.complete
                  ? `${formatCount(result.matched)} matched of ${formatCount(result.scanned)} scanned`
                  : result.matched === 0
                    ? `no matches in the first ${formatCount(result.scanned)} documents (id order) — a full scan covers everything`
                    : `at least ${formatCount(result.matched)} matched in the first ${formatCount(result.scanned)} documents (id order) — a full scan covers everything`}
              </p>
              {result.sample.length > 0 ? (
                <ul className="max-h-64 space-y-1 overflow-y-auto rounded-lg border border-line p-2">
                  {result.sample.map((hit) => (
                    <li key={hit.document_id} className="text-caption">
                      <span className="text-ink">{hit.semantic_id ?? hit.document_id}</span>
                      <span className="ml-2 break-all font-mono text-ink-faint">{hit.matched_on}</span>
                    </li>
                  ))}
                </ul>
              ) : null}
            </div>
          ) : null}
        </div>
      ) : null}
    </Dialog>
  );
}

function CreateRuleDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const create = usePruneRuleCreate();
  const [name, setName] = useState('');
  const [kind, setKind] = useState('url_rule');
  const [pattern, setPattern] = useState('');
  const [confidence, setConfidence] = useState('0.8');

  const valid = name.trim() !== '' && pattern.trim() !== '' && Number(confidence) > 0 && Number(confidence) <= 1;

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="New rule"
      description="Rules start disabled. Preview against live data, then enable."
    >
      <div className="space-y-3">
        <label className="block space-y-1 text-label text-ink-mute">
          name
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="calendar-pages" />
        </label>
        <label className="block space-y-1 text-label text-ink-mute">
          kind
          <Select
            value={kind}
            onValueChange={setKind}
            options={[
              { value: 'url_rule', label: 'URL rule — regex against the document URL' },
              { value: 'tag_rule', label: 'tag rule — regex against tag key=value' },
            ]}
            ariaLabel="Rule kind"
          />
        </label>
        <label className="block space-y-1 text-label text-ink-mute">
          pattern (regex)
          <Input
            value={pattern}
            onChange={(e) => setPattern(e.target.value)}
            placeholder={
              kind === 'tag_rule'
                ? String.raw`^section=(archive|legacy)$`
                : String.raw`/(calendar|events)/\d{4}/\d{2}`
            }
            className="font-mono"
          />
        </label>
        {kind === 'tag_rule' ? <TagKeyHints onPick={setPattern} /> : null}
        <label className="block space-y-1 text-label text-ink-mute">
          confidence (0–1]
          <Input value={confidence} onChange={(e) => setConfidence(e.target.value)} inputMode="decimal" />
        </label>
        <div className="flex justify-end gap-2">
          <Button onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button
            variant="primary"
            disabled={!valid || create.isPending}
            onClick={() =>
              create.mutate(
                {
                  name: name.trim(),
                  kind,
                  body: { pattern: pattern.trim(), confidence: Number(confidence) },
                  enabled: false,
                },
                { onSuccess: () => onOpenChange(false) },
              )
            }
          >
            Create disabled
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

/**
 * The tag vocabulary that actually exists in this corpus.
 *
 * A tag rule is a regex matched against `key=value`, and writing one blind
 * meant guessing at keys nobody had listed anywhere — so the commonest outcome
 * was a rule that previewed to zero and looked broken. The counts matter as
 * much as the keys: a key with two distinct values is a flag, one with 40,000
 * is an identifier and a poor thing to pattern-match on.
 */
function TagKeyHints({ onPick }: { onPick: (pattern: string) => void }) {
  const keys = useQuery(tagKeysQuery(40));
  if (!keys.data || keys.data.length === 0) return null;
  return (
    <div className="space-y-1">
      <span className="text-caption text-ink-mute">
        Tag keys in this corpus — matched as <code className="font-mono">key=value</code>
      </span>
      <div className="flex flex-wrap gap-1.5">
        {keys.data.map((tag) => (
          <button
            key={tag.key}
            type="button"
            onClick={() => onPick(`^${tag.key}=`)}
            className="rounded-full border border-line px-2.5 py-0.5 font-mono text-caption text-ink hover:border-line-2"
            title={`${formatCount(tag.distinct_values)} distinct value${tag.distinct_values === 1 ? '' : 's'}`}
          >
            {tag.key}
            <span className="ml-1 text-ink-faint">{formatCount(tag.distinct_values)}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

function ConfigDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const queryClient = useQueryClient();
  const config = useQuery({ ...pruneConfigQuery, enabled: open });
  const importConfig = usePruneConfigImport();
  const [draft, setDraft] = useState<string | null>(null);

  const text = draft ?? config.data ?? '';

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) setDraft(null);
        onOpenChange(next);
      }}
      title="Detector configuration"
      description="The effective config as YAML. Edit and import; unknown keys are rejected loudly."
      className="md:max-w-2xl"
    >
      <div className="space-y-3">
        {config.isPending && draft === null ? (
          <Skeleton className="h-64" />
        ) : (
          <textarea
            value={text}
            onChange={(event) => setDraft(event.target.value)}
            rows={18}
            spellCheck={false}
            aria-label="Detector configuration YAML"
            className="w-full resize-y rounded-lg border border-line-2 bg-well p-3 font-mono text-caption text-ink outline-none focus-visible:border-gold/50"
          />
        )}
        <div className="flex justify-end gap-2">
          <Button
            onClick={() => {
              void navigator.clipboard.writeText(text);
            }}
          >
            Copy
          </Button>
          <Button
            variant="primary"
            disabled={draft === null || importConfig.isPending}
            onClick={() =>
              importConfig.mutate(text, {
                onSuccess: () => {
                  setDraft(null);
                  void queryClient.invalidateQueries({ queryKey: ['prune', 'config'] });
                },
              })
            }
          >
            <FileUp className="size-4" aria-hidden /> Import
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
