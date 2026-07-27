import React from 'react';
import {
  Folder,
  Globe,
  HardDrive,
  BookOpen,
  MessageSquare,
  Layers,
  CheckCircle2,
  AlertTriangle,
  XCircle,
  Database,
  Search,
  Clock,
  Activity,
} from 'lucide-react';
import { ConnectorSummary } from '../../api/types';

export type ActiveNavView = 'pages' | 'recent' | 'flagged' | 'health' | 'prune';

interface SidebarProps {
  connectors: ConnectorSummary[];
  selectedConnector: number | null;
  onSelectConnector: (id: number | null) => void;
  activeView: ActiveNavView;
  onSelectView: (view: ActiveNavView) => void;
  totalPageCount: number;
}

export const Sidebar: React.FC<SidebarProps> = ({
  connectors,
  selectedConnector,
  onSelectConnector,
  activeView,
  onSelectView,
  totalPageCount,
}) => {
  const [filterText, setFilterText] = React.useState('');

  const filteredConnectors = React.useMemo(() => {
    if (!filterText.trim()) return connectors;
    const lower = filterText.toLowerCase().trim();
    return connectors.filter(
      (c) =>
        c.connector_name.toLowerCase().includes(lower) ||
        c.connector_source.toLowerCase().includes(lower)
    );
  }, [connectors, filterText]);

  const getConnectorIcon = (source: string) => {
    switch (source.toLowerCase()) {
      case 'web':
        return Globe;
      case 'google_drive':
        return HardDrive;
      case 'confluence':
        return BookOpen;
      case 'slack':
        return MessageSquare;
      default:
        return Folder;
    }
  };

  const getStatusBadge = (disabled: boolean, totalPages: number) => {
    if (disabled) {
      return (
        <span title="Disabled">
          <XCircle className="w-3.5 h-3.5 text-rose-500 shrink-0" />
        </span>
      );
    }
    if (totalPages === 0) {
      return (
        <span title="Empty / Inactive">
          <AlertTriangle className="w-3.5 h-3.5 text-amber-400 shrink-0" />
        </span>
      );
    }
    return (
      <span title="Active">
        <CheckCircle2 className="w-3.5 h-3.5 text-emerald-400 shrink-0" />
      </span>
    );
  };

  return (
    <aside className="w-64 bg-[#0A1F13]/90 border-r border-[#143322] flex flex-col justify-between shrink-0 select-none">
      <div className="p-4 space-y-6 overflow-y-auto flex-1">
        {/* Main Navigation Views */}
        <div className="space-y-1">
          <div className="px-3 py-1.5 text-[11px] font-semibold tracking-wider text-emerald-400/80 uppercase">
            Navigation Views
          </div>

          {/* 1. All Index Pages */}
          <button
            onClick={() => {
              onSelectView('pages');
              onSelectConnector(null);
            }}
            className={`w-full flex items-center justify-between px-3 py-2 rounded-xl text-xs font-medium transition ${
              activeView === 'pages' && selectedConnector == null
                ? 'bg-amber-500/20 text-amber-300 font-semibold border border-amber-500/40 shadow-sm'
                : 'text-emerald-100/90 hover:bg-[#123621] hover:text-white'
            }`}
          >
            <div className="flex items-center gap-2.5">
              <Layers className="w-4 h-4 text-amber-400" />
              <span>All Index Pages</span>
            </div>
            <span className="px-2 py-0.5 rounded-full bg-gray-800 text-[10px] text-gray-300 font-mono">
              {totalPageCount.toLocaleString()}
            </span>
          </button>

          {/* 2. Recently Indexed */}
          <button
            onClick={() => {
              onSelectView('recent');
              onSelectConnector(null);
            }}
            className={`w-full flex items-center justify-between px-3 py-2 rounded-xl text-xs font-medium transition ${
              activeView === 'recent'
                ? 'bg-emerald-500/20 text-emerald-300 font-semibold border border-emerald-500/40 shadow-sm'
                : 'text-emerald-100/90 hover:bg-[#123621] hover:text-white'
            }`}
          >
            <div className="flex items-center gap-2.5">
              <Clock className="w-4 h-4 text-emerald-400" />
              <span>Recently Indexed</span>
            </div>
            <span className="px-1.5 py-0.5 rounded bg-emerald-950 text-emerald-300 text-[10px] font-mono border border-emerald-700/50">
              New
            </span>
          </button>

          {/* 3. Flagged & Duplicates */}
          <button
            onClick={() => onSelectView('flagged')}
            className={`w-full flex items-center justify-between px-3 py-2 rounded-xl text-xs font-medium transition ${
              activeView === 'flagged' || activeView === 'prune'
                ? 'bg-rose-500/20 text-rose-300 border border-rose-500/40 font-semibold'
                : 'text-gray-300 hover:bg-gray-800/60 hover:text-white'
            }`}
          >
            <div className="flex items-center gap-2.5">
              <AlertTriangle className="w-4 h-4 text-rose-400" />
              <span>Flagged & Duplicates</span>
            </div>
            <span className="px-1.5 py-0.5 rounded bg-rose-950 text-rose-300 text-[10px] font-mono border border-rose-700/50">
              Prune
            </span>
          </button>

          {/* 4. Connector Health Matrix */}
          <button
            onClick={() => onSelectView('health')}
            className={`w-full flex items-center justify-between px-3 py-2 rounded-xl text-xs font-medium transition ${
              activeView === 'health'
                ? 'bg-indigo-500/20 text-indigo-300 border border-indigo-500/40 font-semibold'
                : 'text-gray-300 hover:bg-gray-800/60 hover:text-white'
            }`}
          >
            <div className="flex items-center gap-2.5">
              <Activity className="w-4 h-4 text-indigo-400" />
              <span>Connector Health</span>
            </div>
            <span className="px-1.5 py-0.5 rounded bg-indigo-950 text-indigo-300 text-[10px] font-mono border border-indigo-700/50">
              Matrix
            </span>
          </button>
        </div>

        {/* Slack-style Connector List with Search Filter */}
        <div className="space-y-2">
          <div className="px-3 py-1.5 text-[11px] font-semibold tracking-wider text-gray-400 uppercase flex items-center justify-between">
            <span>Connectors</span>
            <span className="text-[10px] text-gray-500 font-normal">{connectors.length} total</span>
          </div>

          {connectors.length > 5 && (
            <div className="px-2">
              <div className="flex items-center gap-2 px-2.5 py-1.5 rounded-lg bg-gray-900/90 border border-gray-800 focus-within:border-violet-500/60 transition">
                <Search className="w-3.5 h-3.5 text-gray-500 shrink-0" />
                <input
                  type="text"
                  value={filterText}
                  onChange={(e) => setFilterText(e.target.value)}
                  placeholder="Filter connectors..."
                  className="w-full bg-transparent text-xs text-gray-200 placeholder-gray-500 focus:outline-none"
                />
                {filterText && (
                  <button
                    onClick={() => setFilterText('')}
                    className="text-[10px] text-gray-500 hover:text-gray-300"
                  >
                    ✕
                  </button>
                )}
              </div>
            </div>
          )}

          <div className="max-h-72 overflow-y-auto space-y-1 pr-1">
            {filteredConnectors.length === 0 ? (
              <div className="px-3 py-2 text-xs text-gray-500 italic">No connectors found</div>
            ) : (
              filteredConnectors.map((connector) => {
                const Icon = getConnectorIcon(connector.connector_source);
                const isSelected = selectedConnector === connector.connector_id && activeView === 'pages';

                return (
                  <button
                    key={connector.connector_id}
                    onClick={() => {
                      onSelectView('pages');
                      onSelectConnector(connector.connector_id);
                    }}
                    className={`w-full flex items-center justify-between px-3 py-2 rounded-xl text-xs font-medium transition ${
                      isSelected
                        ? 'bg-violet-600/25 text-violet-300 border border-violet-500/40 font-semibold'
                        : 'text-gray-300 hover:bg-gray-800/60 hover:text-white'
                    }`}
                  >
                    <div className="flex items-center gap-2.5 truncate">
                      {getStatusBadge(connector.disabled, connector.total_pages)}
                      <Icon className="w-3.5 h-3.5 text-gray-400 shrink-0" />
                      <span className="truncate">{connector.connector_name}</span>
                    </div>
                    <span className="px-1.5 py-0.5 rounded text-[10px] text-gray-400 font-mono">
                      {connector.total_pages.toLocaleString()}
                    </span>
                  </button>
                );
              })
            )}
          </div>
        </div>
      </div>

      {/* System Engine Status Footer */}
      <div className="p-4 border-t border-[#143322] bg-[#05140C]/60 space-y-2 text-[11px] text-emerald-400/80">
        <div className="flex items-center justify-between pb-1 border-b border-[#143322]">
          <span className="flex items-center gap-1.5 font-bold text-amber-300">
            <Layers className="w-3.5 h-3.5 text-amber-400" />
            OVIS System
          </span>
          <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-emerald-950 text-amber-300 border border-emerald-700/60 font-bold">
            v0.1.0
          </span>
        </div>

        <div className="flex items-center justify-between">
          <span className="flex items-center gap-1.5">
            <Search className="w-3 h-3 text-emerald-400" />
            OpenSearch Index
          </span>
          <span className="text-emerald-400 font-mono text-[10px]">danswer_chunk</span>
        </div>
        <div className="flex items-center justify-between">
          <span className="flex items-center gap-1.5">
            <Database className="w-3 h-3 text-indigo-400" />
            PostgreSQL Pool
          </span>
          <span className="text-indigo-300 font-mono text-[10px]">public.document</span>
        </div>
      </div>
    </aside>
  );
};
