import React from 'react';
import { ConnectorSummary } from '../../api/types';
import {
  Activity,
  CheckCircle2,
  AlertTriangle,
  XCircle,
  Globe,
  HardDrive,
  BookOpen,
  MessageSquare,
  Folder,
  RefreshCw,
  Database,
  Search,
} from 'lucide-react';

interface ConnectorHealthMatrixProps {
  connectors: ConnectorSummary[];
  onRefresh?: () => void;
}

export const ConnectorHealthMatrix: React.FC<ConnectorHealthMatrixProps> = ({
  connectors,
  onRefresh,
}) => {
  const totalPages = connectors.reduce((acc, c) => acc + c.total_pages, 0);
  const activeCount = connectors.filter((c) => !c.disabled && c.total_pages > 0).length;
  const emptyCount = connectors.filter((c) => !c.disabled && c.total_pages === 0).length;
  const disabledCount = connectors.filter((c) => c.disabled).length;

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

  const formatDate = (isoString?: string) => {
    if (!isoString) return 'Never / Pending';
    try {
      return new Date(isoString).toLocaleString(undefined, {
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      });
    } catch {
      return isoString;
    }
  };

  return (
    <div className="space-y-6 p-2 animate-fadeIn">
      {/* Header Bar */}
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-xl font-bold text-gray-100 flex items-center gap-2">
            <Activity className="w-5 h-5 text-emerald-400" />
            Connector Health Matrix
          </h2>
          <p className="text-xs text-gray-400 mt-1">
            Aggregated real-time synchronization, health status, and index volume across all active data connectors.
          </p>
        </div>

        {onRefresh && (
          <button
            onClick={onRefresh}
            className="px-3.5 py-1.5 rounded-xl bg-[#0A1F13] border border-[#1A422D] hover:border-emerald-500/50 text-xs font-semibold text-emerald-300 hover:text-emerald-100 transition flex items-center gap-2"
          >
            <RefreshCw className="w-3.5 h-3.5" />
            <span>Re-check Health</span>
          </button>
        )}
      </div>

      {/* Aggregate KPI Summary Grid */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
        <div className="p-4 rounded-2xl bg-[#0A1F13]/80 border border-[#143322] space-y-1">
          <div className="text-[11px] font-semibold text-gray-400 uppercase tracking-wider">Total Connectors</div>
          <div className="text-2xl font-bold text-gray-100">{connectors.length}</div>
          <div className="text-[11px] text-emerald-400 font-mono">{activeCount} active operational</div>
        </div>

        <div className="p-4 rounded-2xl bg-[#0A1F13]/80 border border-[#143322] space-y-1">
          <div className="text-[11px] font-semibold text-gray-400 uppercase tracking-wider">Total Pages Indexed</div>
          <div className="text-2xl font-bold text-amber-300 font-mono">{totalPages.toLocaleString()}</div>
          <div className="text-[11px] text-gray-400">Across SQL & OpenSearch</div>
        </div>

        <div className="p-4 rounded-2xl bg-[#0A1F13]/80 border border-[#143322] space-y-1">
          <div className="text-[11px] font-semibold text-gray-400 uppercase tracking-wider">Sync Health Rate</div>
          <div className="text-2xl font-bold text-emerald-400 font-mono">
            {connectors.length > 0
              ? `${Math.round((activeCount / connectors.length) * 100)}%`
              : '100%'}
          </div>
          <div className="text-[11px] text-gray-400">
            {emptyCount} empty / {disabledCount} disabled
          </div>
        </div>

        <div className="p-4 rounded-2xl bg-[#0A1F13]/80 border border-[#143322] space-y-1">
          <div className="text-[11px] font-semibold text-gray-400 uppercase tracking-wider">Primary Storage</div>
          <div className="text-2xl font-bold text-indigo-300 flex items-center gap-2">
            <Database className="w-5 h-5 text-indigo-400" />
            PostgreSQL
          </div>
          <div className="text-[11px] text-indigo-400 font-mono">public.document</div>
        </div>
      </div>

      {/* Connector Matrix Cards Grid */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {connectors.map((connector) => {
          const Icon = getConnectorIcon(connector.connector_source);
          const isOk = !connector.disabled && connector.total_pages > 0;
          const isEmpty = !connector.disabled && connector.total_pages === 0;

          return (
            <div
              key={connector.connector_id}
              className="p-5 rounded-2xl bg-[#0A1F13]/90 border border-[#143322] hover:border-[#1E4D34] transition space-y-4 shadow-xl"
            >
              <div className="flex items-start justify-between">
                <div className="flex items-center gap-3">
                  <div className="p-2.5 rounded-xl bg-gray-900 border border-gray-800 text-emerald-400">
                    <Icon className="w-5 h-5" />
                  </div>
                  <div>
                    <h3 className="font-bold text-sm text-gray-100">{connector.connector_name}</h3>
                    <span className="text-[11px] text-gray-400 font-mono capitalize">
                      Source: {connector.connector_source} (ID #{connector.connector_id})
                    </span>
                  </div>
                </div>

                {connector.disabled ? (
                  <span className="inline-flex items-center gap-1 px-2.5 py-0.5 rounded-full bg-rose-500/20 text-rose-300 border border-rose-500/40 text-[10px] font-semibold">
                    <XCircle className="w-3 h-3" />
                    DISABLED
                  </span>
                ) : isEmpty ? (
                  <span className="inline-flex items-center gap-1 px-2.5 py-0.5 rounded-full bg-amber-500/20 text-amber-300 border border-amber-500/40 text-[10px] font-semibold">
                    <AlertTriangle className="w-3 h-3" />
                    EMPTY
                  </span>
                ) : (
                  <span className="inline-flex items-center gap-1 px-2.5 py-0.5 rounded-full bg-emerald-500/20 text-emerald-300 border border-emerald-500/40 text-[10px] font-semibold">
                    <CheckCircle2 className="w-3 h-3 text-emerald-400" />
                    ACTIVE
                  </span>
                )}
              </div>

              <div className="grid grid-cols-2 gap-2 text-xs border-t border-gray-800 pt-3">
                <div>
                  <span className="text-[10px] text-gray-400 block uppercase">Pages Indexed</span>
                  <span className="font-mono font-bold text-gray-200">{connector.total_pages.toLocaleString()}</span>
                </div>
                <div>
                  <span className="text-[10px] text-gray-400 block uppercase">Last Index Sync</span>
                  <span className="font-mono text-gray-300 text-[11px]">{formatDate(connector.last_indexed_at)}</span>
                </div>
              </div>

              <div className="flex items-center justify-between text-[11px] font-mono text-gray-400 bg-black/40 px-3 py-1.5 rounded-lg border border-gray-800/80">
                <span className="flex items-center gap-1">
                  <Search className="w-3 h-3 text-emerald-400" />
                  Index: danswer_chunk
                </span>
                <span className="text-emerald-400 font-semibold">{isOk ? '100% Synced' : 'Standby'}</span>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
};
