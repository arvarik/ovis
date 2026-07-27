import { createRootRoute, Link } from '@tanstack/react-router';
import { AppShell } from '@/components/shell/AppShell';
import { EmptyState, ErrorState } from '@/components/primitives/EmptyState';

function NotFound() {
  return (
    <EmptyState
      title="This page does not exist"
      description="The address doesn't match any Ovis view."
      action={
        <Link
          to="/pages"
          className="inline-flex min-h-11 items-center justify-center rounded-lg border border-line-2 bg-surface px-4 text-body text-ink transition-colors hover:bg-hover md:min-h-9 md:text-label"
        >
          Back to Pages
        </Link>
      }
    />
  );
}

function RootError({ error, reset }: { error: Error; reset: () => void }) {
  return <ErrorState error={error} onRetry={reset} title="The view crashed" />;
}

export const rootRoute = createRootRoute({
  component: AppShell,
  notFoundComponent: NotFound,
  errorComponent: RootError,
});
