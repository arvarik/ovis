import React from 'react';
import { Layers, AlertCircle, CheckCircle2, Clock } from 'lucide-react';
import { PageListItem } from '../../api/types';
import { ActionsMenu } from './ActionsMenu';

interface TableRowProps {
  page: PageListItem;
  isSelected: boolean;
  onToggleSelect: (id: string) => void;
  onInspect: (page: PageListItem) => void;
  onDelete: (id: string) => void;
  style?: React.CSSProperties;
}

export const TableRow: React.FC<TableRowProps> = ({
  page,
  isSelected,
  onToggleSelect,
  onInspect,
  onDelete,
  style,
}) => {
  const isWarningChunkCount = page.chunk_count === 0 || page.chunk_count > 50;

  const rawDateStr =
    page.doc_updated_at ||
    page.metadata?.doc_updated_at ||
    page.metadata?.updated_at ||
    page.metadata?.last_modified ||
    page.metadata?.created_at ||
    page.metadata?.indexed_at ||
    page.metadata?.timestamp;

  const getRelativeTimeString = (isoString?: string) => {
    if (!isoString || isoString === 'null' || isoString === 'undefined') {
      return { display: 'No timestamp', full: 'No timestamp recorded in document metadata' };
    }
    try {
      const date = new Date(isoString);
      if (isNaN(date.getTime())) return { display: 'No timestamp', full: String(isoString) };
      const now = new Date();
      const diffMs = now.getTime() - date.getTime();
      const diffSec = Math.floor(diffMs / 1000);

      if (diffSec < 0) {
        // Future timestamp or clock drift
        return { display: date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' }), full: date.toISOString() };
      }

      const diffMin = Math.floor(diffSec / 60);
      const diffHour = Math.floor(diffMin / 60);
      const diffDay = Math.floor(diffHour / 24);

      let display = '';
      if (diffSec < 60) display = 'just now';
      else if (diffMin < 60) display = `${diffMin}m ago`;
      else if (diffHour < 24) display = `${diffHour}h ago`;
      else if (diffDay < 30) display = `${diffDay}d ago`;
      else display = date.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' });

      return { display, full: date.toISOString() };
    } catch {
      return { display: 'No timestamp', full: String(isoString) };
    }
  };

  const { display: formattedDate, full: fullIsoDate } = getRelativeTimeString(rawDateStr);

  const connectorDisplayName = page.connector_name || page.connector_source || 'Web';

  return (
    <div
      style={style}
      onClick={() => onInspect(page)}
      className={`group flex items-center px-4 py-3 border-b border-[#143322] text-xs font-medium cursor-pointer transition-colors duration-150 ${
        isSelected
          ? 'bg-rose-500/10 border-rose-500/30'
          : 'hover:bg-[#112A1B]/60'
      }`}
    >
      {/* Checkbox Selection */}
      <div className="w-10 flex items-center justify-center shrink-0" onClick={(e) => e.stopPropagation()}>
        <input
          type="checkbox"
          checked={isSelected}
          onChange={() => onToggleSelect(page.id)}
          className="rounded border-gray-700 bg-gray-900 text-rose-500 focus:ring-rose-500/50 cursor-pointer"
        />
      </div>

      {/* Status Marker Badge */}
      <div className="w-14 flex items-center shrink-0">
        {page.chunk_count > 0 ? (
          <span className="inline-flex items-center gap-1 text-[11px] text-emerald-400 font-semibold" title="Indexed cleanly">
            <CheckCircle2 className="w-3.5 h-3.5" />
            <span>OK</span>
          </span>
        ) : (
          <span className="inline-flex items-center gap-1 text-[11px] text-amber-400 font-semibold" title="Flagged / Stub">
            <AlertCircle className="w-3.5 h-3.5" />
            <span>STUB</span>
          </span>
        )}
      </div>

      {/* Document Title & Semantic ID */}
      <div className="flex-1 min-w-0 pr-4">
        <div className="font-semibold text-emerald-100 truncate group-hover:text-amber-300 transition">
          {page.semantic_id || page.id}
        </div>
        {page.link && (
          <div className="text-[11px] text-emerald-400/70 truncate mt-0.5 font-mono">
            {page.link}
          </div>
        )}
      </div>

      {/* Connector Name (Strict Bounds) */}
      <div className="w-36 shrink-0 pr-3 min-w-0">
        <span
          className="block w-full px-2 py-0.5 rounded-md bg-[#05140C] text-emerald-300 text-[10px] font-mono border border-[#173826] truncate text-center"
          title={connectorDisplayName}
        >
          {connectorDisplayName}
        </span>
      </div>

      {/* Chunks Count */}
      <div className="w-24 shrink-0 flex items-center gap-1 pr-2">
        <Layers className={`w-3.5 h-3.5 ${isWarningChunkCount ? 'text-amber-400' : 'text-indigo-400'}`} />
        <span className={`font-mono text-xs ${isWarningChunkCount ? 'text-amber-300 font-bold' : 'text-emerald-200'}`}>
          {page.chunk_count} {page.chunk_count === 1 ? 'chunk' : 'chunks'}
        </span>
      </div>

      {/* Updated Timestamp */}
      <div
        className="w-28 shrink-0 flex items-center gap-1 text-emerald-400/80 text-[11px] font-mono pr-2"
        title={fullIsoDate}
      >
        <Clock className="w-3 h-3 text-emerald-400 shrink-0" />
        <span>{formattedDate}</span>
      </div>

      {/* Quick Action Buttons */}
      <div className="w-24 shrink-0 flex justify-end">
        <ActionsMenu
          onInspect={() => onInspect(page)}
          onDelete={() => onDelete(page.id)}
          link={page.link}
        />
      </div>
    </div>
  );
};
