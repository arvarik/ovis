import { useState } from 'react';
import { createRoute } from '@tanstack/react-router';
import { toast } from 'sonner';
import { MoreHorizontal, Trash2 } from 'lucide-react';
import { rootRoute } from './__root';
import { Button, IconButton } from '@/components/primitives/Button';
import { Badge, type BadgeTone } from '@/components/primitives/Badge';
import { Kbd } from '@/components/primitives/Kbd';
import { Input } from '@/components/primitives/Input';
import { Card } from '@/components/primitives/Card';
import { Skeleton } from '@/components/primitives/Skeleton';
import { Spinner } from '@/components/primitives/Spinner';
import { EmptyState, ErrorState } from '@/components/primitives/EmptyState';
import { Stat } from '@/components/primitives/Stat';
import { Sheet } from '@/components/primitives/Sheet';
import { Dialog, AlertDialog } from '@/components/primitives/Dialog';
import {
  MenuRoot,
  MenuTrigger,
  MenuContent,
  MenuItem,
  MenuSeparator,
} from '@/components/primitives/Menu';
import { Tooltip } from '@/components/primitives/Tooltip';
import { TabsRoot, TabsList, TabsTrigger, TabsContent } from '@/components/primitives/Tabs';
import { ApiError } from '@/api/client';

const TONES: BadgeTone[] = ['gold', 'mint', 'rose', 'indigo', 'violet', 'teal', 'neutral'];

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="space-y-3">
      <h2 className="font-display font-display-soft text-display text-ink">{title}</h2>
      {children}
    </section>
  );
}

/** Storybook-less demo of every primitive (M1 exit criterion). Not in nav. */
function LabView() {
  const [sheetOpen, setSheetOpen] = useState(false);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [alertOpen, setAlertOpen] = useState(false);

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-3xl space-y-10 p-4 pb-24 md:p-8">
        <Section title="Buttons">
          <div className="flex flex-wrap items-center gap-3">
            <Button variant="primary">Primary</Button>
            <Button variant="secondary">Secondary</Button>
            <Button variant="destructive">Delete</Button>
            <Button variant="ghost">Ghost</Button>
            <Button variant="primary" disabled>
              Disabled
            </Button>
            <IconButton label="More actions">
              <MoreHorizontal className="size-4" aria-hidden />
            </IconButton>
            <Spinner label="Loading" />
          </div>
        </Section>

        <Section title="Badges">
          <div className="flex flex-wrap gap-2">
            {TONES.map((tone) => (
              <Badge key={tone} tone={tone}>
                {tone}
              </Badge>
            ))}
          </div>
        </Section>

        <Section title="Inputs & Kbd">
          <div className="flex max-w-md flex-col gap-3">
            <Input placeholder="Search pages…" />
            <Input mono defaultValue="https://example.com/a-very-long-url/path" />
            <p className="text-body text-ink-mute">
              Press <Kbd>⌘</Kbd> <Kbd>K</Kbd> for the palette, <Kbd>?</Kbd> for help.
            </p>
          </div>
        </Section>

        <Section title="Stat tiles">
          <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
            <Stat label="Documents" value="1,646,781" approximate caption="planner estimate" />
            <Stat label="Chunks" value="10.0M" />
            <Stat label="Index size" value="371.5 GB" caption="71% disk used" tone="gold" />
            <Stat label="Failed" value="1,057" tone="rose" />
          </div>
        </Section>

        <Section title="Cards, skeletons, empty states">
          <div className="grid gap-3 md:grid-cols-2">
            <Card>
              <h3 className="font-display text-title text-ink">A card</h3>
              <p className="mt-1 text-body text-ink-mute">Level-1 surface, 14px radius.</p>
            </Card>
            <Card className="space-y-2">
              <Skeleton className="h-4 w-3/4" delayMs={0} />
              <Skeleton className="h-4 w-1/2" delayMs={0} />
              <Skeleton className="h-4 w-2/3" delayMs={0} />
            </Card>
          </div>
          <Card className="p-0">
            <EmptyState
              title="No pages match"
              description="Filters: connector tildes, chunks ≥ 11."
              action={<Button variant="secondary">Clear filters</Button>}
            />
          </Card>
          <Card className="p-0">
            <ErrorState
              error={new ApiError('DATABASE', 'database error', 500, '01JEXAMPLE')}
              onRetry={() => toast('Retried')}
            />
          </Card>
        </Section>

        <Section title="Tabs">
          <TabsRoot defaultValue="overview">
            <TabsList>
              <TabsTrigger value="overview">Overview</TabsTrigger>
              <TabsTrigger value="text">Text</TabsTrigger>
              <TabsTrigger value="chunks">Chunks</TabsTrigger>
              <TabsTrigger value="json">JSON</TabsTrigger>
            </TabsList>
            <TabsContent value="overview" className="py-4 text-body text-ink-mute">
              Gold underline marks the active tab.
            </TabsContent>
            <TabsContent value="text" className="py-4 text-body text-ink-mute">
              Text content.
            </TabsContent>
            <TabsContent value="chunks" className="py-4 text-body text-ink-mute">
              Chunk cards.
            </TabsContent>
            <TabsContent value="json" className="py-4 text-body text-ink-mute">
              Collapsible tree.
            </TabsContent>
          </TabsRoot>
        </Section>

        <Section title="Overlays">
          <div className="flex flex-wrap gap-3">
            <Button onClick={() => setSheetOpen(true)}>Open sheet</Button>
            <Button onClick={() => setDialogOpen(true)}>Open dialog</Button>
            <Button variant="destructive" onClick={() => setAlertOpen(true)}>
              Confirm delete
            </Button>
            <MenuRoot>
              <MenuTrigger asChild>
                <Button variant="secondary">Open menu</Button>
              </MenuTrigger>
              <MenuContent>
                <MenuItem>Inspect</MenuItem>
                <MenuItem>Copy URL</MenuItem>
                <MenuSeparator />
                <MenuItem destructive icon={<Trash2 aria-hidden />}>
                  Delete
                </MenuItem>
              </MenuContent>
            </MenuRoot>
            <Tooltip content="Tooltips are hover-only hints">
              <Button variant="ghost">Hover me</Button>
            </Tooltip>
            <Button onClick={() => toast.success('Saved', { description: 'Quiet, 3 seconds.' })}>
              Success toast
            </Button>
            <Button
              onClick={() =>
                toast.error('Delete failed', {
                  description: 'OPENSEARCH_UPSTREAM · req 01J…',
                  duration: Infinity,
                })
              }
            >
              Error toast
            </Button>
          </div>

          <Sheet
            open={sheetOpen}
            onOpenChange={setSheetOpen}
            title="Demo sheet"
            description="Bottom sheet on mobile, right panel on desktop"
          >
            <div className="space-y-3 overflow-y-auto p-5">
              <h3 className="font-display font-display-soft text-title text-ink">
                One component, two shapes
              </h3>
              <p className="text-body text-ink-mute">
                Resize the window across 768px — this surface swaps between a
                bottom sheet and a right side panel without remounting different
                component trees.
              </p>
            </div>
          </Sheet>

          <Dialog
            open={dialogOpen}
            onOpenChange={setDialogOpen}
            title="A dialog"
            description="Radix supplies trap, aria, scroll lock"
          >
            <p className="text-body text-ink-mute">
              Escape closes only this layer — one layer at a time.
            </p>
          </Dialog>

          <AlertDialog
            open={alertOpen}
            onOpenChange={setAlertOpen}
            title="Delete this document?"
            actions={
              <Button variant="destructive" onClick={() => setAlertOpen(false)}>
                Delete
              </Button>
            }
          >
            <p>
              14 chunks will be removed. The owning connector is ACTIVE, so a
              recrawl is likely to bring the page back.
            </p>
          </AlertDialog>
        </Section>
      </div>
    </div>
  );
}

export const labRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: 'lab',
  component: LabView,
});
