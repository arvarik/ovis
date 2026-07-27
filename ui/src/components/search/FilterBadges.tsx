import React from 'react';
import { X, Filter } from 'lucide-react';
import { ConnectorSummary } from '../../api/types';

interface FilterBadgesProps {
  selectedConnector: number | null;
  onClearConnector: () => void;
  query: string;
  onClearQuery: () => void;
  connectors: ConnectorSummary[];
}

export const FilterBadges: React.FC<FilterBadgesProps> = ({
  selectedConnector,
  onClearConnector,
  query,
  onClearQuery,
  connectors,
}) => {
  if (selectedConnector == null && !query) return null;

  const currentConnector = connectors.find((c) => c.connector_id === selectedConnector);

  return (
    <div className="flex items-center gap-2 px-4 py-1.5 overflow-x-auto text-xs">
      <span className="flex items-center gap-1 text-gray-400 font-medium shrink-0">
        <Filter className="w-3 h-3 text-rose-400" />
        <span>Active Filters:</span>
      </span>

      {currentConnector && (
        <span className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-violet-600/20 border border-violet-500/40 text-violet-300 text-xs shrink-0">
          <span>Connector: {currentConnector.connector_name}</span>
          <button
            onClick={onClearConnector}
            className="p-0.5 rounded-full hover:bg-violet-500/30 hover:text-white transition"
          >
            <X className="w-3 h-3" />
          </button>
        </span>
      )}

      {query && (
        <span className="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-rose-500/20 border border-rose-500/40 text-rose-300 text-xs shrink-0">
          <span>Search: "{query}"</span>
          <button
            onClick={onClearQuery}
            className="p-0.5 rounded-full hover:bg-rose-500/30 hover:text-white transition"
          >
            <X className="w-3 h-3" />
          </button>
        </span>
      )}
    </div>
  );
};
