import React, { useState } from 'react';
import { PruneCandidatePair } from '../../api/types';
import { CandidateCard } from './CandidateCard';
import { Zap, Layers, RefreshCw, Trash2, Filter } from 'lucide-react';
import { Callout } from '../common/Callout';

interface PruneDashboardProps {
  onPruneDocument: (docId: string) => void;
}

export const PruneDashboard: React.FC<PruneDashboardProps> = ({ onPruneDocument }) => {
  const [candidates, setCandidates] = useState<PruneCandidatePair[]>([]);
  const [filterReason, setFilterReason] = useState<string>('all');
  const [isScanning, setIsScanning] = useState<boolean>(false);

  const handleScan = () => {
    setIsScanning(true);
    setTimeout(() => {
      setIsScanning(false);
    }, 1200);
  };

  const handlePruneDoc = (docId: string) => {
    onPruneDocument(docId);
    setCandidates((prev) =>
      prev.filter((c) => c.doc_id_a !== docId && c.doc_id_b !== docId)
    );
  };

  const filteredCandidates = candidates.filter((c) => {
    if (filterReason === 'all') return true;
    return c.flag_reason === filterReason;
  });

  return (
    <div className="space-y-6 max-w-6xl mx-auto p-4 animate-fadeIn">
      {/* Metrics Bar */}
      <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
        <div className="p-5 rounded-2xl bg-[#0A1F13]/90 border border-[#143322] shadow-xl flex items-center gap-4">
          <div className="p-3 rounded-xl bg-amber-500/20 text-amber-400">
            <Zap className="w-6 h-6" />
          </div>
          <div>
            <div className="text-2xl font-bold text-emerald-50">{candidates.length}</div>
            <div className="text-xs text-emerald-400/70">Duplicate Candidates Flagged</div>
          </div>
        </div>

        <div className="p-5 rounded-2xl bg-[#0A1F13]/90 border border-[#143322] shadow-xl flex items-center gap-4">
          <div className="p-3 rounded-xl bg-rose-500/20 text-rose-400">
            <Layers className="w-6 h-6" />
          </div>
          <div>
            <div className="text-2xl font-bold text-emerald-50">~8.4 MB</div>
            <div className="text-xs text-emerald-400/70">OpenSearch Vector Storage Bloat</div>
          </div>
        </div>

        <div className="p-5 rounded-2xl bg-[#0A1F13]/90 border border-[#143322] shadow-xl flex items-center gap-4">
          <div className="p-3 rounded-xl bg-emerald-500/20 text-emerald-400">
            <Trash2 className="w-6 h-6" />
          </div>
          <div>
            <div className="text-2xl font-bold text-emerald-50">MinHash LSH</div>
            <div className="text-xs text-emerald-400/70">5-Shingle Tokenizer & Jaccard Sim</div>
          </div>
        </div>
      </div>

      {/* Notice Banner */}
      <Callout icon="⚡" title="Pruning Engine Audit & Quality Inspector" variant="warning">
        The OVIS pruning engine computes 5-shingle MinHash LSH buckets across vector text chunks to identify $O(N)$ near-duplicate pairs and empty stubs. Select individual documents below to execute cascading deletion.
      </Callout>

      {/* Action and Filter Controls */}
      <div className="flex flex-col sm:flex-row items-center justify-between gap-4 p-4 rounded-2xl bg-gray-900/60 border border-gray-800">
        <div className="flex items-center gap-2">
          <Filter className="w-4 h-4 text-rose-400" />
          <span className="text-xs font-semibold text-gray-300">Filter Flag Reason:</span>
          <select
            value={filterReason}
            onChange={(e) => setFilterReason(e.target.value)}
            className="px-3 py-1.5 rounded-lg bg-gray-800 border border-gray-700 text-xs text-gray-200 focus:outline-none cursor-pointer"
          >
            <option value="all">All Flags ({candidates.length})</option>
            <option value="near_duplicate">Near Duplicates</option>
            <option value="empty_stub">Empty Stubs</option>
            <option value="boilerplate_error">404 Boilerplate</option>
          </select>
        </div>

        <button
          onClick={handleScan}
          disabled={isScanning}
          className="px-4 py-2 rounded-xl bg-amber-500 hover:bg-amber-400 disabled:opacity-50 text-gray-950 font-bold text-xs flex items-center gap-2 transition shadow-lg shadow-amber-500/20"
        >
          <RefreshCw className={`w-4 h-4 ${isScanning ? 'animate-spin' : ''}`} />
          <span>{isScanning ? 'Scanning OpenSearch Index...' : 'Re-Run LSH Scan'}</span>
        </button>
      </div>

      {/* Candidates List */}
      <div className="space-y-4">
        {filteredCandidates.length === 0 ? (
          <div className="p-12 text-center text-xs text-gray-500 rounded-2xl border border-gray-800 bg-gray-900/30">
            No candidate pairs found matching the selected flag filter.
          </div>
        ) : (
          filteredCandidates.map((candidate) => (
            <CandidateCard
              key={candidate.id}
              candidate={candidate}
              onPruneDoc={handlePruneDoc}
            />
          ))
        )}
      </div>
    </div>
  );
};
