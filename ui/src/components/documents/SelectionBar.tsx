import type { ReactNode } from 'react';
import { Copy, X } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/primitives/Button';
import { IconButton } from '@/components/primitives/Button';
import { count as formatCount } from '@/lib/format';

/**
 * Floating action bar for multi-select. Sits above BottomTabs on mobile,
 * bottom-center on desktop. Extra actions (Hide/Delete) slot in via children.
 */
export function SelectionBar({
  selectedCount,
  urls,
  onClear,
  children,
}: {
  selectedCount: number;
  urls: string[];
  onClear: () => void;
  children?: ReactNode;
}) {
  if (selectedCount === 0) return null;
  return (
    <div className="pointer-events-none fixed inset-x-0 bottom-[calc(72px+env(safe-area-inset-bottom))] z-40 flex justify-center px-4 lg:bottom-6">
      <div className="glass-panel pointer-events-auto flex items-center gap-1.5 rounded-full py-1.5 pr-1.5 pl-4 animate-slide-up">
        <span className="mr-1 text-label text-ink whitespace-nowrap">
          {formatCount(selectedCount)} selected
        </span>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            void navigator.clipboard.writeText(urls.join('\n'));
            toast(`Copied ${formatCount(urls.length)} URLs`);
          }}
        >
          <Copy className="size-4" aria-hidden />
          <span className="hidden sm:inline">Copy URLs</span>
        </Button>
        {children}
        <IconButton label="Clear selection" onClick={onClear}>
          <X className="size-4" aria-hidden />
        </IconButton>
      </div>
    </div>
  );
}
