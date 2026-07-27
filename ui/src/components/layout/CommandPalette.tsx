import React, { useState, useEffect, useRef } from 'react';
import { Search, FileText, Zap, RefreshCw, Layers, ShieldCheck, X } from 'lucide-react';
import { ConnectorSummary } from '../../api/types';

interface CommandPaletteProps {
  isOpen: boolean;
  onClose: () => void;
  onSelectView: (view: 'pages' | 'prune') => void;
  onSelectConnector: (id: number | null) => void;
  onRefresh: () => void;
  connectors: ConnectorSummary[];
}

export const CommandPalette: React.FC<CommandPaletteProps> = ({
  isOpen,
  onClose,
  onSelectView,
  onSelectConnector,
  onRefresh,
  connectors,
}) => {
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState<number>(0);
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);

  useEffect(() => {
    if (isOpen) {
      setQuery('');
      setSelectedIndex(0);
    }
  }, [isOpen]);

  useEffect(() => {
    setSelectedIndex(0);
  }, [query]);

  const commands = [
    {
      id: 'all-pages',
      title: 'Go to All Index Pages',
      subtitle: 'View complete table of document records',
      icon: Layers,
      action: () => {
        onSelectView('pages');
        onSelectConnector(null);
      },
    },
    {
      id: 'prune-dashboard',
      title: 'Run Pruning & Deduplication Inspector',
      subtitle: 'Analyze near-duplicate candidates and empty stubs',
      icon: Zap,
      action: () => {
        onSelectView('prune');
      },
    },
    {
      id: 'refresh-data',
      title: 'Refresh System Connector Statistics',
      subtitle: 'Re-query PostgreSQL & OpenSearch index stats',
      icon: RefreshCw,
      action: () => {
        onRefresh();
      },
    },
    ...connectors.map((c) => ({
      id: `connector-${c.connector_id}`,
      title: `Filter by ${c.connector_name}`,
      subtitle: `${c.total_pages.toLocaleString()} pages indexed (${c.connector_source})`,
      icon: FileText,
      action: () => {
        onSelectView('pages');
        onSelectConnector(c.connector_id);
      },
    })),
  ];

  const filteredCommands = commands.filter(
    (cmd) =>
      cmd.title.toLowerCase().includes(query.toLowerCase()) ||
      cmd.subtitle.toLowerCase().includes(query.toLowerCase())
  );

  useEffect(() => {
    if (itemRefs.current[selectedIndex]) {
      itemRefs.current[selectedIndex]?.scrollIntoView({ block: 'nearest' });
    }
  }, [selectedIndex]);

  if (!isOpen) return null;

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setSelectedIndex((prev) => (filteredCommands.length > 0 ? (prev + 1) % filteredCommands.length : 0));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setSelectedIndex((prev) =>
        filteredCommands.length > 0 ? (prev - 1 + filteredCommands.length) % filteredCommands.length : 0
      );
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (filteredCommands[selectedIndex]) {
        filteredCommands[selectedIndex].action();
        onClose();
      }
    } else if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center pt-20 bg-black/75 backdrop-blur-md transition-opacity"
      onClick={onClose}
    >
      <div
        className="w-full max-w-xl rounded-2xl bg-[#0A1F13] border border-[#1C4730] shadow-2xl overflow-hidden flex flex-col max-h-[70vh] animate-fadeIn"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Search Header */}
        <div className="flex items-center px-4 border-b border-[#143322] bg-[#05140C]/80">
          <Search className="w-5 h-5 text-gray-400 shrink-0" />
          <input
            type="text"
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Type a command or connector name... (e.g. 'prune', 'web', 'refresh')"
            className="w-full px-4 py-4 bg-transparent text-sm text-gray-100 placeholder-gray-400 focus:outline-none font-sans"
          />
          {query && (
            <button
              onClick={() => setQuery('')}
              className="p-1 rounded-full text-gray-400 hover:text-gray-200 hover:bg-gray-800 transition"
            >
              <X className="w-4 h-4" />
            </button>
          )}
        </div>

        {/* Command List */}
        <div className="p-2 space-y-1 overflow-y-auto flex-1">
          {filteredCommands.length === 0 ? (
            <div className="p-6 text-center text-xs text-gray-500">
              No matching commands or connectors found.
            </div>
          ) : (
            filteredCommands.map((cmd, index) => {
              const Icon = cmd.icon;
              const isSelected = index === selectedIndex;
              return (
                <button
                  key={cmd.id}
                  ref={(el) => {
                    itemRefs.current[index] = el;
                  }}
                  onClick={() => {
                    cmd.action();
                    onClose();
                  }}
                  aria-selected={isSelected}
                  className={`w-full flex items-center justify-between px-4 py-3 rounded-xl text-xs font-medium text-left group transition ${
                    isSelected
                      ? 'bg-rose-500/20 text-rose-200 border border-rose-500/50 shadow-md'
                      : 'text-gray-300 hover:bg-rose-500/10 hover:text-rose-200 border border-transparent'
                  }`}
                >
                  <div className="flex items-center gap-3">
                    <div
                      className={`p-2 rounded-lg transition ${
                        isSelected
                          ? 'bg-rose-500/30 text-rose-300'
                          : 'bg-gray-800/80 group-hover:bg-rose-500/30 text-gray-400 group-hover:text-rose-300'
                      }`}
                    >
                      <Icon className="w-4 h-4" />
                    </div>
                    <div>
                      <div
                        className={`font-semibold ${
                          isSelected ? 'text-rose-200' : 'text-gray-100 group-hover:text-rose-200'
                        }`}
                      >
                        {cmd.title}
                      </div>
                      <div className="text-[11px] text-gray-400">{cmd.subtitle}</div>
                    </div>
                  </div>
                  <kbd
                    className={`px-2 py-0.5 rounded text-[10px] font-mono border transition ${
                      isSelected
                        ? 'bg-rose-950/60 text-rose-300 border-rose-500/60'
                        : 'bg-gray-800 text-gray-400 group-hover:text-rose-300 group-hover:bg-rose-950/40 border-gray-700'
                    }`}
                  >
                    Select ↵
                  </kbd>
                </button>
              );
            })
          )}
        </div>

        {/* Footer info */}
        <div className="px-4 py-2 border-t border-gray-800 bg-gray-900/40 text-[11px] text-gray-400 flex items-center justify-between">
          <span className="flex items-center gap-1.5">
            <ShieldCheck className="w-3.5 h-3.5 text-emerald-400" />
            OVIS Quick Switcher
          </span>
          <span>Use ↑ ↓ to navigate, Enter to select, ESC to close</span>
        </div>
      </div>
    </div>
  );
};
