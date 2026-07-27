import React from 'react';
import { PruneCandidatePair } from '../../api/types';
import { Trash2, AlertTriangle } from 'lucide-react';

interface CandidateCardProps {
  candidate: PruneCandidatePair;
  onPruneDoc: (docId: string) => void;
}

export const CandidateCard: React.FC<CandidateCardProps> = ({ candidate, onPruneDoc }) => {
  const getFlagBadge = (reason: string) => {
    switch (reason) {
      case 'near_duplicate':
        return (
          <span className="px-2.5 py-1 rounded-full bg-rose-500/20 text-rose-300 border border-rose-500/40 text-[11px] font-semibold flex items-center gap-1">
            <AlertTriangle className="w-3 h-3 text-rose-400" />
            Near Duplicate ({Math.round(candidate.similarity_score * 100)}% Sim)
          </span>
        );
      case 'empty_stub':
        return (
          <span className="px-2.5 py-1 rounded-full bg-amber-500/20 text-amber-300 border border-amber-500/40 text-[11px] font-semibold">
            Empty Stub Page
          </span>
        );
      case 'boilerplate_error':
        return (
          <span className="px-2.5 py-1 rounded-full bg-violet-500/20 text-violet-300 border border-violet-500/40 text-[11px] font-semibold">
            404 Boilerplate Error
          </span>
        );
      default:
        return (
          <span className="px-2.5 py-1 rounded-full bg-gray-800 text-gray-300 text-[11px] font-semibold">
            Length Anomaly
          </span>
        );
    }
  };

  return (
    <div className="p-5 rounded-2xl bg-gray-900/80 border border-gray-800 hover:border-gray-700 transition shadow-xl space-y-4">
      {/* Top Header */}
      <div className="flex items-center justify-between">
        {getFlagBadge(candidate.flag_reason)}
        <div className="text-[11px] font-mono text-gray-400">
          Shingle Overlap: <strong className="text-gray-200">{candidate.shingle_overlap_percent}%</strong>
        </div>
      </div>

      {/* Side-by-side comparison */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-4 pt-2">
        {/* Document A */}
        <div className="p-4 rounded-xl bg-black/50 border border-gray-800/80 space-y-2">
          <div className="flex items-center justify-between text-xs">
            <span className="font-semibold text-rose-400">Candidate A</span>
            <span className="text-[10px] font-mono text-gray-400">{candidate.connector_a}</span>
          </div>
          <h4 className="font-bold text-sm text-gray-100 truncate" title={candidate.title_a}>
            {candidate.title_a}
          </h4>
          <div className="text-[11px] text-gray-400 font-mono truncate">{candidate.doc_id_a}</div>
          <button
            onClick={() => onPruneDoc(candidate.doc_id_a)}
            className="w-full mt-2 px-3 py-1.5 rounded-lg bg-rose-500/10 hover:bg-rose-500/20 text-rose-400 text-xs font-semibold flex items-center justify-center gap-1.5 border border-rose-500/30 transition"
          >
            <Trash2 className="w-3.5 h-3.5" />
            <span>Prune Doc A</span>
          </button>
        </div>

        {/* Document B */}
        <div className="p-4 rounded-xl bg-black/50 border border-gray-800/80 space-y-2">
          <div className="flex items-center justify-between text-xs">
            <span className="font-semibold text-indigo-400">Candidate B</span>
            <span className="text-[10px] font-mono text-gray-400">{candidate.connector_b}</span>
          </div>
          <h4 className="font-bold text-sm text-gray-100 truncate" title={candidate.title_b}>
            {candidate.title_b}
          </h4>
          <div className="text-[11px] text-gray-400 font-mono truncate">{candidate.doc_id_b}</div>
          <button
            onClick={() => onPruneDoc(candidate.doc_id_b)}
            className="w-full mt-2 px-3 py-1.5 rounded-lg bg-indigo-500/10 hover:bg-indigo-500/20 text-indigo-400 text-xs font-semibold flex items-center justify-center gap-1.5 border border-indigo-500/30 transition"
          >
            <Trash2 className="w-3.5 h-3.5" />
            <span>Prune Doc B</span>
          </button>
        </div>
      </div>
    </div>
  );
};
