/**
 * Ask the assigned model to name the groups on screen.
 *
 * Deliberately a button rather than something that happens on load. Narration
 * costs model calls, and a page that quietly spends them every time it renders
 * is a page nobody can reason about. Re-running is safe and cheap: subjects
 * already titled by this model and prompt are skipped server-side, so pressing
 * it twice titles only what is new.
 */
import { Sparkles } from 'lucide-react';
import { useQuery } from '@tanstack/react-query';
import { llmRolesQuery } from '@/api/queries';
import { useNarrate } from '@/api/mutations';
import { Button } from '@/components/primitives/Button';

export function NarrateButton({
  subjectKind,
  method,
  disabled,
}: {
  subjectKind: 'cluster' | 'bundle';
  method?: string;
  disabled?: boolean;
}) {
  const narrate = useNarrate();
  const roles = useQuery(llmRolesQuery);

  // The roles endpoint 503s on a deployment with no LLM tables, which is a
  // supported state, not an error: the button simply is not offered.
  if (roles.isError || !roles.data?.narrate) return null;

  return (
    <Button
      size="sm"
      variant="secondary"
      className="whitespace-nowrap"
      disabled={disabled || narrate.isPending}
      onClick={() => narrate.mutate({ subject_kind: subjectKind, method })}
      title={`Uses ${roles.data.narrate.display_name ?? roles.data.narrate.model_id}`}
    >
      <Sparkles aria-hidden className="size-3.5" />
      {narrate.isPending ? 'Writing titles…' : 'Write titles'}
    </Button>
  );
}
