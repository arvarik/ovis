import { useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { versionQuery } from '@/api/queries';
import { Dialog } from '@/components/primitives/Dialog';
import { Input } from '@/components/primitives/Input';
import { Kbd } from '@/components/primitives/Kbd';
import { comboLabel, useHotkeyList } from '@/hooks/hotkeys';

/**
 * The `?` overlay — rendered from the same registry that dispatches keys, so
 * help cannot drift from behavior. Searchable, grouped.
 */
export function HelpOverlay({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [filter, setFilter] = useState('');
  const bindings = useHotkeyList();

  const groups = useMemo(() => {
    const visible = bindings.filter((b) => !b.hidden && b.description !== '');
    const needle = filter.trim().toLowerCase();
    const filtered = needle
      ? visible.filter(
          (b) =>
            b.description.toLowerCase().includes(needle) ||
            b.keys.toLowerCase().includes(needle) ||
            b.group.toLowerCase().includes(needle),
        )
      : visible;
    const byGroup = new Map<string, typeof filtered>();
    for (const b of filtered) {
      const list = byGroup.get(b.group) ?? [];
      list.push(b);
      byGroup.set(b.group, list);
    }
    return [...byGroup.entries()];
  }, [bindings, filter]);

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) setFilter('');
        onOpenChange(o);
      }}
      title="Keyboard shortcuts"
      description="Bindings for the current screen"
    >
      <Input
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
        placeholder="Filter shortcuts…"
        aria-label="Filter shortcuts"
        className="mb-4"
      />
      {groups.length === 0 ? (
        <p className="py-6 text-center text-label text-ink-faint">No shortcuts match.</p>
      ) : (
        <div className="space-y-5">
          {groups.map(([group, list]) => (
            <section key={group}>
              <h3 className="mb-1.5 text-label font-medium text-ink-faint">{group}</h3>
              <ul className="divide-y divide-line/60">
                {list.map((b) => (
                  <li key={b.id} className="flex items-center justify-between gap-4 py-2">
                    <span className="text-body text-ink-mute">{b.description}</span>
                    <span className="flex shrink-0 items-center gap-1">
                      {comboLabel(b.keys).map((part, i) => (
                        <Kbd key={i}>{part}</Kbd>
                      ))}
                    </span>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </div>
      )}
      <BuildStamp />
    </Dialog>
  );
}

/**
 * Which build is answering.
 *
 * The backend has always served this and nothing showed it, so "is the fix
 * deployed?" was a question you answered by ssh. It belongs here rather than in
 * a corner of the chrome: it is reference information, wanted rarely and
 * exactly when something looks wrong.
 */
function BuildStamp() {
  const build = useQuery(versionQuery);
  if (!build.data) return null;
  const { version, git_sha, profile, built_at } = build.data;
  return (
    <p className="mt-5 border-t border-line pt-3 text-caption text-ink-faint">
      OVIS {version}
      {git_sha ? ` · ${git_sha.slice(0, 7)}` : ''}
      {profile && profile !== 'release' ? ` · ${profile}` : ''}
      {built_at ? ` · built ${built_at}` : ''}
    </p>
  );
}
