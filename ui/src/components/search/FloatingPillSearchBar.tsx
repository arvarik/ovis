import React, { useRef, useEffect, useState } from 'react';
import { Search, SlidersHorizontal, X, Check, Filter } from 'lucide-react';
import { ConnectorSummary } from '../../api/types';

interface FloatingPillSearchBarProps {
  query: string;
  onSearchChange: (query: string) => void;
  selectedConnector: number | null;
  onSelectConnector: (connectorId: number | null) => void;
  connectors: ConnectorSummary[];
  statusFilter?: string | null;
  onSelectStatusFilter?: (status: string | null) => void;
  sortOrder?: string;
  onSelectSortOrder?: (order: string) => void;
  chunkRangeFilter?: string | null;
  onSelectChunkRange?: (range: string | null) => void;
  inputRef?: React.RefObject<HTMLInputElement | null>;
}

export const FloatingPillSearchBar: React.FC<FloatingPillSearchBarProps> = ({
  query,
  onSearchChange,
  selectedConnector,
  onSelectConnector,
  connectors,
  statusFilter,
  onSelectStatusFilter,
  sortOrder = 'updated_desc',
  onSelectSortOrder,
  chunkRangeFilter,
  onSelectChunkRange,
  inputRef: externalInputRef,
}) => {
  const localInputRef = useRef<HTMLInputElement>(null);
  const refToUse = externalInputRef || localInputRef;
  const [isPopoverOpen, setIsPopoverOpen] = useState(false);
  const popoverRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (
        e.key === '/' &&
        document.activeElement?.tagName !== 'INPUT' &&
        document.activeElement?.tagName !== 'TEXTAREA'
      ) {
        e.preventDefault();
        refToUse.current?.focus();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [refToUse]);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (popoverRef.current && !popoverRef.current.contains(event.target as Node)) {
        setIsPopoverOpen(false);
      }
    };
    if (isPopoverOpen) {
      document.addEventListener('mousedown', handleClickOutside);
    }
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, [isPopoverOpen]);

  const activeFilterCount =
    (selectedConnector !== null ? 1 : 0) +
    (statusFilter ? 1 : 0) +
    (query ? 1 : 0) +
    (chunkRangeFilter ? 1 : 0) +
    (sortOrder !== 'updated_desc' ? 1 : 0);

  const currentConnector = connectors.find((c) => c.connector_id === selectedConnector);

  return (
    <div className="relative z-40 flex flex-col items-center justify-center w-full my-2 px-4 space-y-2">
      {/* Primary Floating Pill Bar */}
      <div className="relative flex items-center gap-3 w-full max-w-3xl px-4 py-2.5 rounded-full bg-[#0A1F13]/95 border border-[#143322] shadow-2xl backdrop-blur-2xl focus-within:border-[#10B981]/80 focus-within:ring-2 focus-within:ring-[#10B981]/30 transition-all duration-300">
        <Search className="w-4 h-4 text-emerald-400 shrink-0 ml-1" />

        <input
          ref={refToUse}
          type="text"
          value={query}
          onChange={(e) => onSearchChange(e.target.value)}
          placeholder="Search document title, URL, or ID... (Press '/' to focus)"
          className="flex-1 bg-transparent text-xs text-emerald-50 placeholder-emerald-400/60 focus:outline-none font-sans"
        />

        {query ? (
          <button
            onClick={() => onSearchChange('')}
            className="p-1 rounded-full text-gray-400 hover:text-gray-200 hover:bg-gray-800 transition"
            title="Clear search query"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        ) : (
          <kbd className="hidden sm:inline-block px-1.5 py-0.5 rounded bg-[#05140C] text-[10px] font-mono border border-[#1E4D34] text-amber-300">
            /
          </kbd>
        )}

        <div className="h-4 w-[1px] bg-[#143322]" />

        {/* Filter Popover Panel Trigger Button */}
        <div className="relative" ref={popoverRef}>
          <button
            onClick={() => setIsPopoverOpen((prev) => !prev)}
            className={`flex items-center gap-2 px-3 py-1 rounded-full text-xs font-medium transition ${
              isPopoverOpen || activeFilterCount > 0
                ? 'bg-amber-500/20 text-amber-300 border border-amber-500/50 font-semibold'
                : 'bg-[#05140C] text-emerald-200 hover:text-white border border-[#173826]'
            }`}
            title="Open Filter Controls"
          >
            <SlidersHorizontal className="w-3.5 h-3.5 text-amber-400" />
            <span>Filters</span>
            {activeFilterCount > 0 && (
              <span className="px-1.5 py-0.2 rounded-full bg-amber-500 text-black text-[10px] font-bold">
                {activeFilterCount}
              </span>
            )}
          </button>

          {/* Filter Popover Panel */}
          {isPopoverOpen && (
            <div className="absolute right-0 mt-3 w-80 rounded-2xl bg-[#0A1F13] border border-[#1C4730] shadow-2xl p-4 z-50 space-y-4 animate-fadeIn">
              <div className="flex items-center justify-between border-b border-[#143322] pb-2">
                <span className="text-xs font-semibold text-emerald-300 flex items-center gap-1.5">
                  <Filter className="w-3.5 h-3.5 text-amber-400" />
                  Filter & Sort Options
                </span>
                {activeFilterCount > 0 && (
                  <button
                    onClick={() => {
                      onSelectConnector(null);
                      if (onSelectStatusFilter) onSelectStatusFilter(null);
                      if (onSelectSortOrder) onSelectSortOrder('updated_desc');
                      if (onSelectChunkRange) onSelectChunkRange(null);
                      onSearchChange('');
                    }}
                    className="text-[11px] text-amber-400 hover:underline font-medium"
                  >
                    Clear All
                  </button>
                )}
              </div>

              {/* Sort Order Selector */}
              {onSelectSortOrder && (
                <div className="space-y-1.5">
                  <div className="text-[11px] font-semibold text-emerald-400/80 uppercase tracking-wider">
                    Sort Document Order
                  </div>
                  <div className="grid grid-cols-2 gap-1">
                    {[
                      { id: 'updated_desc', label: 'Updated (Newest)' },
                      { id: 'updated_asc', label: 'Updated (Oldest)' },
                      { id: 'chunks_desc', label: 'Chunks (High-Low)' },
                      { id: 'chunks_asc', label: 'Chunks (Low-High)' },
                      { id: 'title_asc', label: 'Title (A-Z)' },
                    ].map((s) => (
                      <button
                        key={s.id}
                        onClick={() => onSelectSortOrder(s.id)}
                        className={`px-2 py-1 rounded-lg text-[11px] font-medium border text-left truncate transition ${
                          sortOrder === s.id
                            ? 'bg-amber-500/20 text-amber-300 border-amber-500/50 font-bold'
                            : 'bg-[#05140C] text-emerald-200 hover:text-white border-[#173826]'
                        }`}
                      >
                        {s.label}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              {/* Chunk Count Range Filter */}
              {onSelectChunkRange && (
                <div className="space-y-1.5 border-t border-[#143322] pt-3">
                  <div className="text-[11px] font-semibold text-emerald-400/80 uppercase tracking-wider">
                    Chunk Count Range
                  </div>
                  <div className="flex flex-wrap gap-1">
                    {[
                      { id: null, label: 'All' },
                      { id: 'stub', label: '0 (Stub)' },
                      { id: 'small', label: '1-5' },
                      { id: 'medium', label: '6-20' },
                      { id: 'heavy', label: '>20' },
                    ].map((cr) => (
                      <button
                        key={cr.label}
                        onClick={() => onSelectChunkRange(cr.id)}
                        className={`px-2.5 py-1 rounded-full text-[11px] font-medium border transition ${
                          chunkRangeFilter === cr.id
                            ? 'bg-emerald-500/20 text-emerald-300 border-emerald-500/50 font-semibold'
                            : 'bg-[#05140C] text-emerald-200 hover:text-white border-[#173826]'
                        }`}
                      >
                        {cr.label}
                      </button>
                    ))}
                  </div>
                </div>
              )}

              {/* Connector Options */}
              <div className="space-y-1.5 border-t border-[#143322] pt-3">
                <div className="text-[11px] font-semibold text-emerald-400/80 uppercase tracking-wider">
                  Connector Source
                </div>
                <div className="max-h-36 overflow-y-auto space-y-1 pr-1">
                  <button
                    onClick={() => onSelectConnector(null)}
                    className={`w-full flex items-center justify-between px-2.5 py-1.5 rounded-lg text-xs transition ${
                      selectedConnector === null
                        ? 'bg-amber-500/20 text-amber-300 font-semibold border border-amber-500/40'
                        : 'text-emerald-200 hover:bg-[#112A1B]'
                    }`}
                  >
                    <span>All Connectors</span>
                    {selectedConnector === null && <Check className="w-3.5 h-3.5 text-amber-400" />}
                  </button>

                  {connectors.map((c) => (
                    <button
                      key={c.connector_id}
                      onClick={() => onSelectConnector(c.connector_id)}
                      className={`w-full flex items-center justify-between px-2.5 py-1.5 rounded-lg text-xs transition ${
                        selectedConnector === c.connector_id
                          ? 'bg-amber-500/20 text-amber-300 font-semibold border border-amber-500/40'
                          : 'text-emerald-200 hover:bg-[#112A1B]'
                      }`}
                    >
                      <span className="truncate">{c.connector_name}</span>
                      <span className="px-1.5 py-0.5 rounded bg-[#05140C] text-[10px] font-mono text-emerald-400 ml-2 shrink-0">
                        {c.total_pages}
                      </span>
                    </button>
                  ))}
                </div>
              </div>

              {/* Document Status Filter */}
              {onSelectStatusFilter && (
                <div className="space-y-1.5 border-t border-[#143322] pt-3">
                  <div className="text-[11px] font-semibold text-emerald-400/80 uppercase tracking-wider">
                    Document Status
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {[
                      { id: null, label: 'All' },
                      { id: 'ok', label: 'Indexed (OK)' },
                      { id: 'stub', label: 'STUB / Warning' },
                    ].map((st) => (
                      <button
                        key={st.label}
                        onClick={() => onSelectStatusFilter(st.id)}
                        className={`px-2.5 py-1 rounded-full text-xs font-medium border transition ${
                          statusFilter === st.id
                            ? 'bg-amber-500/20 text-amber-300 border-amber-500/50'
                            : 'bg-[#05140C] text-emerald-200 hover:text-white border-[#173826]'
                        }`}
                      >
                        {st.label}
                      </button>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      </div>

      {/* Integrated Active Scope Badges */}
      {activeFilterCount > 0 && (
        <div className="flex flex-wrap items-center justify-center gap-2 text-xs animate-fadeIn">
          {currentConnector && (
            <span className="inline-flex items-center gap-1.5 px-3 py-0.5 rounded-full bg-violet-600/20 border border-violet-500/40 text-violet-300 text-xs">
              <span>Connector: {currentConnector.connector_name}</span>
              <button
                onClick={() => onSelectConnector(null)}
                className="p-0.5 rounded-full hover:bg-violet-500/30 hover:text-white transition"
                title="Remove connector filter"
              >
                <X className="w-3 h-3" />
              </button>
            </span>
          )}

          {statusFilter && (
            <span className="inline-flex items-center gap-1.5 px-3 py-0.5 rounded-full bg-amber-500/20 border border-amber-500/40 text-amber-300 text-xs">
              <span>Status: {statusFilter.toUpperCase()}</span>
              <button
                onClick={() => onSelectStatusFilter && onSelectStatusFilter(null)}
                className="p-0.5 rounded-full hover:bg-amber-500/30 hover:text-white transition"
                title="Remove status filter"
              >
                <X className="w-3 h-3" />
              </button>
            </span>
          )}

          {query && (
            <span className="inline-flex items-center gap-1.5 px-3 py-0.5 rounded-full bg-emerald-500/20 border border-emerald-500/40 text-emerald-300 text-xs">
              <span>Search: "{query}"</span>
              <button
                onClick={() => onSearchChange('')}
                className="p-0.5 rounded-full hover:bg-emerald-500/30 hover:text-white transition"
                title="Clear search text"
              >
                <X className="w-3 h-3" />
              </button>
            </span>
          )}
        </div>
      )}
    </div>
  );
};
