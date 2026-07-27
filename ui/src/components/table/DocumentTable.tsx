import React, { useRef, useState, useEffect } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { PageListItem } from '../../api/types';
import { TableRow } from './TableRow';
import { Trash2, AlertCircle, Layers, Settings, Zap, RefreshCw, Download, ArrowUp, ArrowDown, ArrowUpDown } from 'lucide-react';

interface DocumentTableProps {
  pages: PageListItem[];
  total: number;
  loading: boolean;
  onInspect: (page: PageListItem) => void;
  onDelete: (id: string) => void;
  onBatchDelete: (ids: string[]) => void;
  page: number;
  limit: number;
  onPageChange: (newPage: number) => void;
  onLimitChange?: (newLimit: number) => void;
  useSSE?: boolean;
  onToggleSSE?: (enabled: boolean) => void;
  streamStats?: { time_ms: number } | null;
  onRefresh?: () => void;
  sortOrder?: string;
  onSelectSortOrder?: (order: string) => void;
}

export const DocumentTable: React.FC<DocumentTableProps> = ({
  pages,
  total,
  loading,
  onInspect,
  onDelete,
  onBatchDelete,
  page,
  limit,
  onPageChange,
  onLimitChange,
  useSSE = false,
  onToggleSSE,
  streamStats,
  onRefresh,
  sortOrder = 'updated_desc',
  onSelectSortOrder,
}) => {
  const parentRef = useRef<HTMLDivElement>(null);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [autoRefreshInterval, setAutoRefreshInterval] = useState<number>(0);
  const settingsRef = useRef<HTMLDivElement>(null);

  const rowVirtualizer = useVirtualizer({
    count: pages.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 54,
    overscan: 10,
  });

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (settingsRef.current && !settingsRef.current.contains(event.target as Node)) {
        setIsSettingsOpen(false);
      }
    };
    if (isSettingsOpen) {
      document.addEventListener('mousedown', handleClickOutside);
    }
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, [isSettingsOpen]);

  useEffect(() => {
    if (autoRefreshInterval <= 0 || !onRefresh) return;
    const interval = setInterval(() => {
      onRefresh();
    }, autoRefreshInterval * 1000);
    return () => clearInterval(interval);
  }, [autoRefreshInterval, onRefresh]);

  const toggleSelectAll = () => {
    if (selectedIds.size === pages.length && pages.length > 0) {
      setSelectedIds(new Set());
    } else {
      setSelectedIds(new Set(pages.map((p) => p.id)));
    }
  };

  const toggleSelect = (id: string) => {
    const next = new Set(selectedIds);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    setSelectedIds(next);
  };

  const handleBatchDeleteClick = () => {
    if (selectedIds.size === 0) return;
    onBatchDelete(Array.from(selectedIds));
    setSelectedIds(new Set());
  };

  const handleExportJSON = () => {
    const jsonStr = JSON.stringify(pages, null, 2);
    const blob = new Blob([jsonStr], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `ovis_pages_export_${Date.now()}.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const handleExportCSV = () => {
    const headers = ['ID', 'Semantic ID', 'Link', 'Connector ID', 'Connector Name', 'Chunk Count', 'Updated At'];
    const rows = pages.map((p) => [
      `"${p.id.replace(/"/g, '""')}"`,
      `"${(p.semantic_id || '').replace(/"/g, '""')}"`,
      `"${(p.link || '').replace(/"/g, '""')}"`,
      p.connector_id ?? '',
      `"${(p.connector_name || p.connector_source || '').replace(/"/g, '""')}"`,
      p.chunk_count,
      `"${p.doc_updated_at || ''}"`,
    ]);
    const csvContent = [headers.join(','), ...rows.map((r) => r.join(','))].join('\n');
    const blob = new Blob([csvContent], { type: 'text/csv' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `ovis_pages_export_${Date.now()}.csv`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const handleCopySelectedURLs = () => {
    const selectedPages = pages.filter((p) => selectedIds.has(p.id));
    const urls = selectedPages.map((p) => p.link || p.id).join('\n');
    navigator.clipboard.writeText(urls);
  };

  const totalPages = Math.ceil(total / limit) || 1;

  return (
    <div className="flex flex-col h-full overflow-hidden bg-[#0A1F13]/90 rounded-2xl border border-[#143322] shadow-2xl backdrop-blur-xl">
      {/* Table Control Header Bar */}
      <div className="px-5 py-2.5 bg-[#05140C]/90 border-b border-[#143322] flex items-center justify-between shrink-0">
        <div className="flex items-center gap-3">
          <span className="text-xs font-semibold text-emerald-200 tracking-wide flex items-center gap-2">
            <Layers className="w-4 h-4 text-emerald-400" />
            Document Records Index ({pages.length.toLocaleString()} items)
          </span>

          {useSSE && (
            <span className="inline-flex items-center gap-1.5 px-2.5 py-0.5 rounded-full bg-rose-500/20 text-rose-300 border border-rose-500/40 text-[10px] font-mono">
              <Zap className="w-3 h-3 text-rose-400 animate-pulse" />
              <span>SSE Live {streamStats ? `(${streamStats.time_ms}ms)` : ''}</span>
            </span>
          )}
        </div>

        <div className="flex items-center gap-2">
          <button
            onClick={handleExportJSON}
            className="flex items-center gap-1 px-2.5 py-1 rounded-lg bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-300 border border-emerald-500/40 text-xs font-semibold transition"
            title="Export filtered table to JSON"
          >
            <Download className="w-3 h-3 text-emerald-400" />
            <span>JSON</span>
          </button>

          <button
            onClick={handleExportCSV}
            className="flex items-center gap-1 px-2.5 py-1 rounded-lg bg-teal-500/20 hover:bg-teal-500/30 text-teal-300 border border-teal-500/40 text-xs font-semibold transition"
            title="Export filtered table to CSV"
          >
            <Download className="w-3 h-3 text-teal-400" />
            <span>CSV</span>
          </button>

          {/* Table View Settings Popover */}
          <div className="relative" ref={settingsRef}>
            <button
              onClick={() => setIsSettingsOpen((prev) => !prev)}
              className="flex items-center gap-1.5 px-3 py-1 rounded-lg bg-[#05140C] hover:bg-[#112A1B] border border-[#173826] text-xs font-medium text-emerald-200 transition"
              title="Table View Settings"
            >
              <Settings className="w-3.5 h-3.5 text-amber-400" />
              <span>Table Settings</span>
            </button>

            {isSettingsOpen && (
              <div className="absolute right-0 mt-2 w-72 rounded-2xl bg-[#0A1F13] border border-[#1C4730] shadow-2xl p-4 z-50 space-y-4 animate-fadeIn">
                <div className="border-b border-[#143322] pb-2 flex items-center justify-between">
                  <span className="text-xs font-semibold text-amber-300 flex items-center gap-1.5">
                    <Settings className="w-3.5 h-3.5 text-amber-400" />
                    Table View Settings
                  </span>
                  {onRefresh && (
                    <button
                      onClick={() => {
                        onRefresh();
                        setIsSettingsOpen(false);
                      }}
                      className="p-1 rounded text-emerald-400 hover:text-white hover:bg-[#112A1B] transition"
                      title="Manual Refresh"
                    >
                      <RefreshCw className="w-3.5 h-3.5" />
                    </button>
                  )}
                </div>

                {/* Streaming Mode Toggle */}
                {onToggleSSE && (
                  <div className="space-y-1.5">
                    <div className="text-[11px] font-semibold text-emerald-400/80 uppercase tracking-wider">
                      Data Streaming Mode
                    </div>
                    <div className="grid grid-cols-2 gap-1.5">
                      <button
                        onClick={() => onToggleSSE(false)}
                        className={`px-2.5 py-1.5 rounded-lg text-xs font-medium border text-center transition ${
                          !useSSE
                            ? 'bg-emerald-500/20 text-emerald-300 border-emerald-500/50 font-semibold'
                            : 'bg-[#05140C] text-emerald-300 border-[#173826]'
                        }`}
                      >
                        REST (JSON)
                      </button>
                      <button
                        onClick={() => onToggleSSE(true)}
                        className={`px-2.5 py-1.5 rounded-lg text-xs font-medium border text-center flex items-center justify-center gap-1 transition ${
                          useSSE
                            ? 'bg-rose-500/20 text-rose-300 border-rose-500/50 font-semibold'
                            : 'bg-[#05140C] text-emerald-300 border-[#173826]'
                        }`}
                      >
                        <Zap className="w-3 h-3 text-rose-400" />
                        <span>SSE Live</span>
                      </button>
                    </div>
                  </div>
                )}

                {/* Page Size Selector */}
                {onLimitChange && (
                  <div className="space-y-1.5 border-t border-[#143322] pt-3">
                    <div className="text-[11px] font-semibold text-emerald-400/80 uppercase tracking-wider">
                      Rows Per Page
                    </div>
                    <div className="grid grid-cols-4 gap-1">
                      {[25, 50, 100, 250].map((pageSize) => (
                        <button
                          key={pageSize}
                          onClick={() => onLimitChange(pageSize)}
                          className={`py-1 rounded-lg text-xs font-mono font-medium border text-center transition ${
                            limit === pageSize
                              ? 'bg-amber-500/20 text-amber-300 border-amber-500/50 font-bold'
                              : 'bg-[#05140C] text-emerald-300 border-[#173826]'
                          }`}
                        >
                          {pageSize}
                        </button>
                      ))}
                    </div>
                  </div>
                )}

                {/* Auto Refresh Setting */}
                <div className="space-y-1.5 border-t border-[#143322] pt-3">
                  <div className="text-[11px] font-semibold text-emerald-400/80 uppercase tracking-wider">
                    Auto Refresh Controls
                  </div>
                  <div className="grid grid-cols-4 gap-1">
                    {[
                      { label: 'Off', sec: 0 },
                      { label: '10s', sec: 10 },
                      { label: '30s', sec: 30 },
                      { label: '60s', sec: 60 },
                    ].map((item) => (
                      <button
                        key={item.label}
                        onClick={() => setAutoRefreshInterval(item.sec)}
                        className={`py-1 rounded-lg text-xs font-mono font-medium border text-center transition ${
                          autoRefreshInterval === item.sec
                            ? 'bg-violet-500/20 text-violet-300 border-violet-500/50 font-bold'
                            : 'bg-[#05140C] text-emerald-300 border-[#173826]'
                        }`}
                      >
                        {item.label}
                      </button>
                    ))}
                  </div>
                </div>
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Table Action Bar for Batch Selection */}
      {selectedIds.size > 0 && (
        <div className="px-6 py-2.5 bg-rose-950/40 border-b border-rose-800/60 flex items-center justify-between animate-fadeIn">
          <div className="text-xs font-semibold text-rose-200 flex items-center gap-2">
            <AlertCircle className="w-4 h-4 text-rose-400" />
            <span>{selectedIds.size} document(s) selected</span>
          </div>

          <div className="flex items-center gap-2">
            <button
              onClick={handleCopySelectedURLs}
              className="px-3 py-1 rounded-lg bg-emerald-700/80 hover:bg-emerald-600 text-white text-xs font-medium flex items-center gap-1.5 shadow-md transition"
            >
              <span>Copy Selected URLs</span>
            </button>

            <button
              onClick={handleBatchDeleteClick}
              className="px-3 py-1 rounded-lg bg-rose-600 hover:bg-rose-500 text-white text-xs font-medium flex items-center gap-1.5 shadow-md transition"
            >
              <Trash2 className="w-3.5 h-3.5" />
              <span>Batch Delete Selected</span>
            </button>
          </div>
        </div>
      )}

      {/* Table Header */}
      <div className="flex items-center px-4 py-3 bg-[#05140C]/90 border-b border-[#143322] text-[11px] font-semibold text-emerald-400/90 tracking-wider uppercase shrink-0 select-none">
        <div className="w-10 flex items-center justify-center shrink-0">
          <input
            type="checkbox"
            checked={selectedIds.size === pages.length && pages.length > 0}
            onChange={toggleSelectAll}
            className="rounded border-gray-700 bg-gray-900 text-rose-500 focus:ring-rose-500/50 cursor-pointer"
          />
        </div>

        <div className="w-14 shrink-0">Status</div>

        {/* Title Sort Column */}
        <div className="flex-1 pr-4">
          <button
            onClick={() => onSelectSortOrder?.(sortOrder === 'title_asc' ? 'updated_desc' : 'title_asc')}
            className="flex items-center gap-1 hover:text-white transition group/btn"
          >
            <span>Document Title & URL</span>
            {sortOrder === 'title_asc' ? (
              <ArrowUp className="w-3 h-3 text-amber-400" />
            ) : (
              <ArrowUpDown className="w-3 h-3 text-emerald-600 group-hover/btn:text-emerald-300" />
            )}
          </button>
        </div>

        {/* Connector Sort Column */}
        <div className="w-36 shrink-0 pr-2">
          <button
            onClick={() => onSelectSortOrder?.(sortOrder === 'connector_asc' ? 'updated_desc' : 'connector_asc')}
            className="flex items-center gap-1 hover:text-white transition group/btn"
          >
            <span>Connector</span>
            {sortOrder === 'connector_asc' ? (
              <ArrowUp className="w-3 h-3 text-amber-400" />
            ) : (
              <ArrowUpDown className="w-3 h-3 text-emerald-600 group-hover/btn:text-emerald-300" />
            )}
          </button>
        </div>

        {/* Chunks Sort Column */}
        <div className="w-24 shrink-0 pr-2">
          <button
            onClick={() => onSelectSortOrder?.(sortOrder === 'chunks_desc' ? 'chunks_asc' : 'chunks_desc')}
            className="flex items-center gap-1 hover:text-white transition group/btn"
          >
            <span>Chunks</span>
            {sortOrder === 'chunks_desc' ? (
              <ArrowDown className="w-3 h-3 text-amber-400" />
            ) : sortOrder === 'chunks_asc' ? (
              <ArrowUp className="w-3 h-3 text-amber-400" />
            ) : (
              <ArrowUpDown className="w-3 h-3 text-emerald-600 group-hover/btn:text-emerald-300" />
            )}
          </button>
        </div>

        {/* Updated Date Sort Column */}
        <div className="w-28 shrink-0 pr-2">
          <button
            onClick={() => onSelectSortOrder?.(sortOrder === 'updated_desc' ? 'updated_asc' : 'updated_desc')}
            className="flex items-center gap-1 hover:text-white transition group/btn"
          >
            <span>Updated</span>
            {sortOrder === 'updated_desc' ? (
              <ArrowDown className="w-3 h-3 text-amber-400" />
            ) : sortOrder === 'updated_asc' ? (
              <ArrowUp className="w-3 h-3 text-amber-400" />
            ) : (
              <ArrowUpDown className="w-3 h-3 text-emerald-600 group-hover/btn:text-emerald-300" />
            )}
          </button>
        </div>

        <div className="w-24 shrink-0 text-right">Actions</div>
      </div>

      {/* Virtualized Table Scroll Container */}
      <div ref={parentRef} className="flex-1 overflow-y-auto relative">
        {loading && pages.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-64 space-y-3 text-emerald-400 text-xs">
            <div className="w-6 h-6 border-2 border-emerald-400 border-t-transparent rounded-full animate-spin" />
            <span>Fetching document pages...</span>
          </div>
        ) : pages.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-64 space-y-3 text-emerald-400/80 text-xs">
            <Layers className="w-10 h-10 text-emerald-600" />
            <span className="font-medium text-emerald-200">No documents found matching your filter criteria.</span>
            <span>Try searching for a different keyword or resetting connector filters.</span>
          </div>
        ) : (
          <div
            style={{
              height: `${rowVirtualizer.getTotalSize()}px`,
              width: '100%',
              position: 'relative',
            }}
          >
            {rowVirtualizer.getVirtualItems().map((virtualRow) => {
              const pageItem = pages[virtualRow.index];
              return (
                <TableRow
                  key={pageItem.id}
                  page={pageItem}
                  isSelected={selectedIds.has(pageItem.id)}
                  onToggleSelect={toggleSelect}
                  onInspect={onInspect}
                  onDelete={onDelete}
                  style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    width: '100%',
                    height: `${virtualRow.size}px`,
                    transform: `translateY(${virtualRow.start}px)`,
                  }}
                />
              );
            })}
          </div>
        )}
      </div>

      {/* Footer Pagination */}
      <div className="px-6 py-3 bg-[#05140C]/90 border-t border-[#143322] text-xs text-emerald-300 flex items-center justify-between shrink-0">
        <div>
          Showing <span className="font-bold text-amber-300">{pages.length}</span> of{' '}
          <span className="font-bold text-amber-300">{total.toLocaleString()}</span> pages
        </div>

        <div className="flex items-center gap-2">
          <button
            disabled={page <= 1}
            onClick={() => onPageChange(page - 1)}
            className="px-3 py-1 rounded-lg bg-[#0A1F13] hover:bg-[#112A1B] border border-[#173826] disabled:opacity-40 disabled:cursor-not-allowed text-emerald-200 transition"
          >
            Previous
          </button>
          <span className="px-2 font-mono text-emerald-200">
            Page {page} of {totalPages}
          </span>
          <button
            disabled={page >= totalPages}
            onClick={() => onPageChange(page + 1)}
            className="px-3 py-1 rounded-lg bg-[#0A1F13] hover:bg-[#112A1B] border border-[#173826] disabled:opacity-40 disabled:cursor-not-allowed text-emerald-200 transition"
          >
            Next
          </button>
        </div>
      </div>
    </div>
  );
};
