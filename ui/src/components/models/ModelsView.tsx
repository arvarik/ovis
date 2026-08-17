/**
 * /models — connect an LLM endpoint, see what its models can actually do, and
 * assign the three roles.
 *
 * The organising idea is that **nothing here is taken on trust**. A provider's
 * own listing tells you a model exists; only a probe tells you whether its
 * output constraints hold, and a model that fails cannot be given work. So the
 * screen shows findings ("enum ✓ schema ✗ logprobs ✗") rather than badges, and
 * an unprobed model is visibly distinct from one probed and found incapable —
 * those are different states and conflating them would be the whole bug.
 */
import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { llmModelsQuery, llmProvidersQuery, llmRolesQuery } from '@/api/queries';
import {
  useLlmAssignRole,
  useLlmProbe,
  useLlmProbeAll,
  useLlmProviderCreate,
  useLlmProviderDelete,
  useLlmProviderDiscover,
} from '@/api/mutations';
import type { LlmCapabilities, LlmModel, LlmProvider, LlmRole } from '@/api/types';
import { Badge } from '@/components/primitives/Badge';
import { Button } from '@/components/primitives/Button';
import { Card } from '@/components/primitives/Card';
import { Dialog } from '@/components/primitives/Dialog';
import { EmptyState } from '@/components/primitives/EmptyState';
import { Input } from '@/components/primitives/Input';
import { Select } from '@/components/primitives/Select';
import { Skeleton } from '@/components/primitives/Skeleton';
import { count as formatCount } from '@/lib/format';

const KINDS = [
  { value: 'llamacpp', label: 'llama.cpp server' },
  { value: 'openai_compatible', label: 'OpenAI-compatible (vLLM, LM Studio, …)' },
  { value: 'ollama', label: 'Ollama' },
  { value: 'gemini', label: 'Google Gemini' },
  { value: 'anthropic', label: 'Anthropic' },
];

const HOSTED = new Set(['gemini', 'anthropic']);

/** What each role is for, in one line, because the names alone do not say. */
const ROLE_COPY: { id: LlmRole; name: string; blurb: string }[] = [
  {
    id: 'bulk',
    name: 'Bulk judging',
    blurb:
      'Grades many documents. Prefer a local endpoint — it is free, your documents stay on your network, and it is usually the only tier that returns a confidence distribution.',
  },
  {
    id: 'quality',
    name: 'Quality judging',
    blurb:
      'Spot-checks and calibration. Disagreement between this and the bulk model is what tells you whether either can be trusted.',
  },
  {
    id: 'narrate',
    name: 'Narration',
    blurb:
      'Writes the titles and summaries that turn a long candidate list into a few named decisions. A few thousand calls in total.',
  },
];

export function ModelsView() {
  const providers = useQuery(llmProvidersQuery);
  const models = useQuery(llmModelsQuery());
  const roles = useQuery(llmRolesQuery);
  const [connecting, setConnecting] = useState(false);

  if (providers.isPending) return <Skeleton className="h-64 w-full" />;
  if (providers.isError) {
    return (
      <EmptyState
        title="Models unavailable"
        description={providers.error?.message ?? 'The provider list could not be loaded.'}
      />
    );
  }

  const items = providers.data?.items ?? [];

  return (
    <div className="h-full overflow-y-auto overscroll-contain">
      <div className="mx-auto w-full max-w-6xl space-y-6 p-4 pb-24 md:p-6 md:pb-24">
        <header className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h1 className="font-display font-display-soft text-headline text-ink">Models</h1>
            <p className="mt-1 max-w-2xl text-label text-ink-mute">
              Connect any endpoint that serves an LLM — your own box or a hosted API. OVIS lists
              what it offers, then tests each model to see which output constraints actually
              hold. Only models that pass can be given work.
            </p>
          </div>
          <Button onClick={() => setConnecting(true)}>Connect an endpoint</Button>
        </header>

        {items.length === 0 ? (
          <EmptyState
            title="No endpoints connected"
            description="Point OVIS at a local llama.cpp or Ollama server, an OpenAI-compatible endpoint, or a hosted API. Nothing in pruning requires one — this only adds relevance judging and narration."
            action={<Button onClick={() => setConnecting(true)}>Connect an endpoint</Button>}
          />
        ) : (
          <>
            <RolesPanel roles={roles.data} models={models.data?.items ?? []} />
            <section className="space-y-3">
              <h2 className="font-display font-display-soft text-title text-ink">Endpoints</h2>
              {items.map((provider) => (
                <ProviderCard
                  key={provider.id}
                  provider={provider}
                  models={(models.data?.items ?? []).filter(
                    (m) => m.provider_id === provider.id,
                  )}
                />
              ))}
            </section>
          </>
        )}

        {connecting ? <ConnectDialog onClose={() => setConnecting(false)} /> : null}
      </div>
    </div>
  );
}

function RolesPanel({
  roles,
  models,
}: {
  roles: Record<string, { provider_name: string; model_id: string } | null> | undefined;
  models: LlmModel[];
}) {
  const assign = useLlmAssignRole();
  // Only models that passed a probe may be offered. This is the same rule the
  // server enforces; showing an ineligible option would just produce an error.
  const eligible = models.filter((m) => usable(m.capabilities));
  const ambiguous = repeatedLabels(eligible);

  return (
    <section className="space-y-3">
      <div>
        <h2 className="font-display font-display-soft text-title text-ink">Roles</h2>
        <p className="mt-1 text-label text-ink-mute">
          Three jobs with different cost and quality profiles, so they are chosen separately.
          One model can hold several.
        </p>
      </div>
      <div className="grid gap-3 lg:grid-cols-3">
        {ROLE_COPY.map((role) => {
          const current = roles?.[role.id] ?? null;
          const value = current ? modelKey(current.provider_name, current.model_id) : '';
          return (
            <Card key={role.id} className="flex flex-col gap-2 p-4">
              <div className="font-display text-ink">{role.name}</div>
              <p className="text-caption text-ink-mute">{role.blurb}</p>
              <div className="mt-auto pt-2">
                <Select
                  value={value}
                  ariaLabel={`Model for ${role.name}`}
                  onValueChange={(next) => {
                    if (!next) {
                      assign.mutate({ role: role.id });
                      return;
                    }
                    const model = eligible.find(
                      (m) => modelKey(m.provider_name, m.model_id) === next,
                    );
                    if (model) {
                      assign.mutate({
                        role: role.id,
                        provider_id: model.provider_id,
                        model_id: model.model_id,
                      });
                    }
                  }}
                  options={[
                    { value: '', label: 'Not assigned' },
                    ...eligible.map((m) => ({
                      value: modelKey(m.provider_name, m.model_id),
                      // A duplicate label here would be worse than in
                      // the list below: picking the wrong one is silent.
                      label: ambiguous.has(label(m))
                        ? `${m.provider_name} · ${label(m)} (${m.model_id})`
                        : `${m.provider_name} · ${label(m)}`,
                    })),
                  ]}
                />
              </div>
            </Card>
          );
        })}
      </div>
      {eligible.length === 0 ? (
        <p className="text-caption text-gold">
          No model has passed a probe yet, so no role can be assigned. Probe an endpoint's
          models below.
        </p>
      ) : null}
    </section>
  );
}

function ProviderCard({ provider, models }: { provider: LlmProvider; models: LlmModel[] }) {
  const probeAll = useLlmProbeAll();
  const remove = useLlmProviderDelete();
  const discover = useLlmProviderDiscover();
  const [expanded, setExpanded] = useState(models.length <= 8);

  const judges = models.filter((m) => usable(m.capabilities)).length;
  const unprobed = models.filter((m) => !m.capabilities).length;

  // A hosted provider lists dozens of models, most of them irrelevant. Order
  // by how much the reader is likely to care: models doing a job, then models
  // that could, then ones already ruled out, then the unprobed remainder.
  const ranked = [...models].sort((a, b) => rank(a) - rank(b));
  const visible = expanded ? ranked : ranked.slice(0, 8);
  const ambiguous = repeatedLabels(models);

  return (
    <Card className="space-y-3 p-4">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div>
          <div className="flex items-center gap-2">
            <span className="font-display text-ink">{provider.name}</span>
            <Badge tone="neutral">{provider.kind.replace(/_/g, ' ')}</Badge>
            {provider.api_key_ref && !provider.key_present ? (
              <Badge tone="rose">{provider.api_key_ref} is not set</Badge>
            ) : null}
          </div>
          <div className="mt-1 text-caption text-ink-mute">
            {provider.base_url ?? 'default endpoint'} ·{' '}
            {formatCount(models.length)} model{models.length === 1 ? '' : 's'} ·{' '}
            {formatCount(judges)} can judge
            {unprobed > 0 ? ` · ${formatCount(unprobed)} unprobed` : ''}
          </div>
        </div>
        <div className="flex gap-2">
          <Button
            size="sm"
            variant="secondary"
            disabled={probeAll.isPending}
            onClick={() => probeAll.mutate(provider.id)}
          >
            {probeAll.isPending ? 'Probing…' : 'Probe all'}
          </Button>
          {/* An endpoint's catalogue moves under it — a model pulled locally, a
              hosted provider retiring one. Re-reading it keeps the roles
              assigned; removing and re-adding the provider threw them away. */}
          <Button
            size="sm"
            variant="secondary"
            disabled={discover.isPending}
            onClick={() => discover.mutate(provider.id)}
            title="Ask this endpoint what it serves now"
          >
            {discover.isPending ? 'Reading…' : 'Refresh models'}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => remove.mutate(provider.id)}
            disabled={remove.isPending}
          >
            Remove
          </Button>
        </div>
      </div>

      <ul className="divide-y divide-line/60">
        {visible.map((model) => (
          <ModelRow
            key={model.model_id}
            model={model}
            showId={ambiguous.has(label(model))}
          />
        ))}
      </ul>

      {models.length > visible.length ? (
        <Button size="sm" variant="ghost" onClick={() => setExpanded(true)}>
          Show {formatCount(models.length - visible.length)} more
        </Button>
      ) : null}
    </Card>
  );
}

function ModelRow({ model, showId }: { model: LlmModel; showId: boolean }) {
  const probe = useLlmProbe();
  const caps = model.capabilities;
  const embedding = model.advertised?.is_embedding ?? false;

  return (
    <li className="flex flex-wrap items-center justify-between gap-2 py-2">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="break-all text-ink">{label(model)}</span>
          {/* Providers reuse one display name across several ids (a dated
              snapshot and its `-latest` alias). Where that happens the name
              alone cannot identify the model, so show the id too. */}
          {showId ? (
            <span className="break-all text-caption text-ink-mute">{model.model_id}</span>
          ) : null}
          {model.roles.map((role) => (
            <Badge key={role} tone="gold">
              {role}
            </Badge>
          ))}
        </div>
        <div className="mt-0.5 text-caption text-ink-mute">
          {embedding ? (
            'embedding model — cannot judge documents'
          ) : caps ? (
            <CapabilitySummary caps={caps} />
          ) : (
            'not probed yet'
          )}
        </div>
        {/* Capped and de-duplicated: a model that fails every probe for the
            same reason produces four near-identical lines, which buries the
            one sentence that explains what to do about it. */}
        {dedupeNotes(caps?.notes).map((note) => (
          <div key={note} className="mt-0.5 text-caption text-gold">
            {note}
          </div>
        ))}
      </div>
      {!embedding ? (
        <Button
          size="sm"
          variant="secondary"
          disabled={probe.isPending}
          onClick={() =>
            probe.mutate({ providerId: model.provider_id, modelId: model.model_id })
          }
        >
          {caps ? 'Re-probe' : 'Probe'}
        </Button>
      ) : null}
    </li>
  );
}

/**
 * Findings, not badges. Each item names what was tested and what happened, so
 * "logprobs ✗" reads as a measurement rather than a defect — plenty of good
 * models simply do not expose them.
 */
function CapabilitySummary({ caps }: { caps: LlmCapabilities }) {
  const items: { label: string; ok: boolean }[] = [
    { label: 'enum', ok: caps.enum_enforced },
    { label: 'schema', ok: caps.schema_enforced },
    { label: 'logprobs', ok: caps.logprobs },
  ];
  return (
    <span className="flex flex-wrap items-center gap-x-3 gap-y-0.5">
      {items.map((item) => (
        <span key={item.label} className={item.ok ? 'text-mint' : 'text-ink-faint'}>
          {item.label} {item.ok ? '✓' : '✗'}
        </span>
      ))}
      {!usable(caps) ? (
        <span className="text-rose">cannot judge documents</span>
      ) : caps.logprobs ? (
        <span className="text-ink-mute">returns a confidence distribution</span>
      ) : null}
    </span>
  );
}

function usable(caps: LlmCapabilities | null): boolean {
  return !!caps && (caps.enum_enforced || caps.schema_enforced);
}

/** The name a provider gives a model, falling back to its id. */
function label(model: LlmModel): string {
  return model.display_name ?? model.model_id;
}

/**
 * A Select option value has to be a single string, and a model is identified by
 * a pair. JSON encodes that unambiguously whatever the provider name or model
 * id contains — no separator can appear inside either half and collide.
 */
function modelKey(providerName: string, modelId: string): string {
  return JSON.stringify([providerName, modelId]);
}

/** Display names shared by more than one model, which therefore identify none. */
function repeatedLabels(models: LlmModel[]): Set<string> {
  const seen = new Set<string>();
  const repeated = new Set<string>();
  for (const m of models) {
    const name = label(m);
    if (seen.has(name)) repeated.add(name);
    seen.add(name);
  }
  return repeated;
}

/** Sort key: holding a role < usable < ruled out < unprobed < embedding. */
function rank(model: LlmModel): number {
  if (model.advertised?.is_embedding) return 4;
  if (model.roles.length > 0) return 0;
  if (usable(model.capabilities)) return 1;
  if (model.capabilities) return 2;
  return 3;
}

/**
 * Keep the explanatory note and at most one representative failure.
 *
 * When every probe fails for one underlying reason — an endpoint returning 404
 * for a retired model, say — the notes repeat that reason once per probe. The
 * sentence worth reading is the conclusion.
 */
function dedupeNotes(notes: string[] | undefined): string[] {
  if (!notes || notes.length === 0) return [];
  const conclusion = notes.filter((n) => n.includes('cannot be used as a judge'));
  const reasons = notes.filter((n) => !conclusion.includes(n));
  const seen = new Set<string>();
  const distinct = reasons.filter((n) => {
    // Collapse "one-of constraint rejected: HTTP 404" and "schema rejected:
    // HTTP 404" onto their shared cause.
    const cause = n.split(': ').slice(1).join(': ') || n;
    if (seen.has(cause)) return false;
    seen.add(cause);
    return true;
  });
  return [...distinct.slice(0, 2), ...conclusion];
}

function ConnectDialog({ onClose }: { onClose: () => void }) {
  const create = useLlmProviderCreate();
  const [kind, setKind] = useState('llamacpp');
  const [name, setName] = useState('');
  const [baseUrl, setBaseUrl] = useState('');
  const [keyRef, setKeyRef] = useState('');
  const hosted = HOSTED.has(kind);

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()} title="Connect an endpoint">
      <div className="space-y-3">
        <div className="text-label text-ink-mute">
          Endpoint type
          <div className="mt-1">
            <Select
              value={kind}
              onValueChange={setKind}
              ariaLabel="Endpoint type"
              options={KINDS}
            />
          </div>
        </div>
        <label className="block text-label text-ink-mute">
          Name
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="local-gemma"
            className="mt-1"
          />
        </label>

        {/* Gemini and Anthropic have one fixed endpoint each; everything else
            is somewhere you have to say. */}
        {!hosted ? (
          <label className="block text-label text-ink-mute">
            Endpoint URL
            <Input
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              placeholder="http://192.168.1.10:8080"
              className="mt-1"
            />
          </label>
        ) : null}

        {/* A key is not only a hosted-API concern: an OpenAI-compatible URL is
            just as often a gateway that wants one. Required there, optional
            here — a llama.cpp box on your own network usually wants nothing. */}
        <label className="block text-label text-ink-mute">
          Environment variable holding the API key
          {!hosted ? <span className="text-ink-mute"> (optional)</span> : null}
          <Input
            value={keyRef}
            onChange={(e) => setKeyRef(e.target.value)}
            placeholder={hosted ? 'OVIS_GEMINI_API_KEY' : 'OVIS_VLLM_API_KEY'}
            className="mt-1"
          />
          <span className="mt-1 block text-caption text-ink-mute">
            The <em>name</em> of the variable, not the key. OVIS reads it from the server's
            environment when it makes a call, so the key is never written to the database
            or included in a backup.
            {!hosted ? ' Leave blank if the endpoint needs no key.' : ''}
          </span>
        </label>

        <div className="flex justify-end gap-2 pt-1">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={
              create.isPending || !name || (hosted ? !keyRef : !baseUrl)
            }
            onClick={() =>
              create.mutate(
                {
                  name,
                  kind,
                  ...(hosted ? {} : { base_url: baseUrl }),
                  ...(keyRef ? { api_key_ref: keyRef } : {}),
                },
                { onSuccess: onClose },
              )
            }
          >
            {create.isPending ? 'Connecting…' : 'Connect'}
          </Button>
        </div>
        <p className="text-caption text-ink-mute">
          Connecting lists the endpoint's models. It does not probe them — that runs a few
          small completions per model and is a separate, explicit step.
        </p>
      </div>
    </Dialog>
  );
}
