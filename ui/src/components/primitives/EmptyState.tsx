import type { ReactNode } from 'react';
import { CircleAlert } from 'lucide-react';
import { cn } from '@/lib/cn';
import { ApiError } from '@/api/client';
import { Button } from './Button';

export interface EmptyStateProps {
  /** Fraunces headline — never a bare "No documents found". */
  title: string;
  description?: ReactNode;
  action?: ReactNode;
  icon?: ReactNode;
  className?: string;
}

export function EmptyState({ title, description, action, icon, className }: EmptyStateProps) {
  return (
    <div
      className={cn(
        'flex flex-col items-center justify-center gap-3 px-6 py-16 text-center',
        className,
      )}
    >
      {icon ? <div className="text-ink-faint [&>svg]:size-8">{icon}</div> : null}
      <h2 className="font-display font-display-soft text-title text-ink">{title}</h2>
      {description ? (
        <div className="max-w-md text-body text-ink-mute">{description}</div>
      ) : null}
      {action ? <div className="mt-2 flex items-center gap-2">{action}</div> : null}
    </div>
  );
}

/**
 * A failed request is an error state with a retry — never an empty list
 * rendered as success. Shows the API's own message, code and req_id.
 */
export function ErrorState({
  error,
  onRetry,
  title = 'Something went wrong',
  className,
}: {
  error: unknown;
  onRetry?: () => void;
  title?: string;
  className?: string;
}) {
  const apiError = error instanceof ApiError ? error : null;
  const message =
    apiError?.message ?? (error instanceof Error ? error.message : 'Unknown error');

  return (
    <EmptyState
      className={className}
      icon={<CircleAlert aria-hidden />}
      title={title}
      description={
        <div className="space-y-1">
          <p>{message}</p>
          {apiError ? (
            <p className="font-mono text-caption text-ink-faint">
              {apiError.code}
              {apiError.reqId ? ` · req ${apiError.reqId}` : ''}
            </p>
          ) : null}
        </div>
      }
      action={
        onRetry ? (
          <Button variant="secondary" onClick={onRetry}>
            Retry
          </Button>
        ) : undefined
      }
    />
  );
}
