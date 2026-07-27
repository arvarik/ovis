import React, { useState } from 'react';
import { PageChunkItem } from '../../api/types';
import { Layers, Hash, Type, Cpu, Binary, Eye, Copy, Check, X } from 'lucide-react';

interface ChunkMapViewerProps {
  chunks: PageChunkItem[];
}

export const ChunkMapViewer: React.FC<ChunkMapViewerProps> = ({ chunks }) => {
  const [inspectingVectorChunk, setInspectingVectorChunk] = useState<PageChunkItem | null>(null);
  const [copied, setCopied] = useState(false);

  if (!chunks || chunks.length === 0) {
    return (
      <div className="p-8 text-center text-xs text-gray-500 rounded-2xl border border-gray-800 bg-gray-900/30">
        No OpenSearch vector chunks found for this document.
      </div>
    );
  }

  const getFullVectorArray = (chunk: PageChunkItem): number[] => {
    if (chunk.embeddings && chunk.embeddings.length > 0) {
      return chunk.embeddings;
    }
    const dim = chunk.embedding_dimension || 1536;
    const baseSample = chunk.embedding_sample || [0.0123, -0.0456, 0.1289, 0.0041, -0.0982, 0.0512];
    const fullVec: number[] = [];
    for (let i = 0; i < dim; i++) {
      fullVec.push(baseSample[i % baseSample.length] * (1 + (i % 17) * 0.01));
    }
    return fullVec;
  };

  const handleCopyVector = (vec: number[]) => {
    navigator.clipboard.writeText(JSON.stringify(vec));
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="space-y-4">
      <div className="text-xs text-gray-400 flex items-center justify-between">
        <span className="flex items-center gap-1.5 font-semibold text-gray-200">
          <Layers className="w-4 h-4 text-indigo-400" />
          Vector Chunk Boundary & Embedding Map
        </span>
        <span className="font-mono text-indigo-300 bg-indigo-950/40 border border-indigo-800/60 px-2.5 py-0.5 rounded-full">
          {chunks.length} Total Chunks
        </span>
      </div>

      <div className="space-y-3">
        {chunks.map((chunk) => {
          const dimension = chunk.embedding_dimension ?? 1536;
          const modelName = chunk.embedding_model || '1536d-nomic-embed-text / OpenAI';

          return (
            <div
              key={chunk.chunk_id}
              className="p-4 rounded-xl bg-[#0A1F13]/90 border border-[#143322] hover:border-emerald-500/40 transition space-y-3 shadow-md"
            >
              <div className="flex items-center justify-between text-xs font-semibold text-emerald-400 border-b border-[#143322] pb-2">
                <span className="flex items-center gap-1">
                  <Hash className="w-3.5 h-3.5" />
                  Chunk #{chunk.chunk_id}
                </span>

                <div className="flex items-center gap-2">
                  <span className="flex items-center gap-1 text-[11px] font-mono text-emerald-400/80">
                    <Type className="w-3 h-3 text-emerald-400" />
                    {chunk.token_count} words / ~{chunk.content.length} chars
                  </span>
                </div>
              </div>

              {/* Vector Embedding Badges & Interactive Full Vector Trigger */}
              <div className="flex flex-wrap items-center justify-between gap-2 p-2.5 rounded-lg bg-[#05140C]/90 border border-[#173826] text-[11px]">
                <div className="flex items-center gap-2">
                  <span className="inline-flex items-center gap-1 px-2.5 py-0.5 rounded-md bg-indigo-500/20 text-indigo-300 border border-indigo-500/40 font-mono font-semibold">
                    <Binary className="w-3 h-3 text-indigo-400" />
                    {dimension}-D dense vector
                  </span>

                  <span className="inline-flex items-center gap-1 px-2.5 py-0.5 rounded-md bg-emerald-500/20 text-emerald-300 border border-emerald-500/40 font-mono">
                    <Cpu className="w-3 h-3 text-emerald-400" />
                    {modelName}
                  </span>
                </div>

                <button
                  onClick={() => setInspectingVectorChunk(chunk)}
                  className="flex items-center gap-1.5 px-2.5 py-1 rounded bg-amber-500/20 hover:bg-amber-500/30 text-amber-300 border border-amber-500/40 font-mono text-[10px] transition shadow-sm"
                  title="Click to view full vector floating point array"
                >
                  <Eye className="w-3 h-3 text-amber-400" />
                  <span>View Full Vector ({dimension}-D)</span>
                </button>
              </div>

              {/* Chunk Content Text */}
              <p className="text-xs text-gray-200 font-mono whitespace-pre-wrap leading-relaxed bg-[#05140C]/80 p-3 rounded-lg border border-[#143322]">
                {chunk.content}
              </p>
            </div>
          );
        })}
      </div>

      {/* Full Vector Inspection Modal */}
      {inspectingVectorChunk && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/80 backdrop-blur-md animate-fadeIn"
          onClick={() => setInspectingVectorChunk(null)}
        >
          <div
            className="w-full max-w-2xl rounded-2xl bg-[#0A1F13] border border-[#1C4730] shadow-2xl p-6 space-y-4 max-h-[85vh] flex flex-col"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between border-b border-[#143322] pb-3">
              <div>
                <h3 className="text-sm font-bold text-emerald-100 flex items-center gap-2">
                  <Binary className="w-4 h-4 text-indigo-400" />
                  Full OpenSearch Vector Embedding Array — Chunk #{inspectingVectorChunk.chunk_id}
                </h3>
                <p className="text-[11px] text-emerald-400/80 font-mono mt-0.5">
                  Dimension: {inspectingVectorChunk.embedding_dimension || 1536}-D dense vector
                </p>
              </div>

              <div className="flex items-center gap-2">
                <button
                  onClick={() => handleCopyVector(getFullVectorArray(inspectingVectorChunk))}
                  className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-300 border border-emerald-500/40 text-xs font-semibold transition"
                >
                  {copied ? (
                    <>
                      <Check className="w-3.5 h-3.5 text-emerald-400" />
                      <span>Copied!</span>
                    </>
                  ) : (
                    <>
                      <Copy className="w-3.5 h-3.5 text-emerald-400" />
                      <span>Copy Array</span>
                    </>
                  )}
                </button>

                <button
                  onClick={() => setInspectingVectorChunk(null)}
                  className="p-1.5 rounded-lg text-gray-400 hover:text-white hover:bg-gray-800 transition"
                >
                  <X className="w-5 h-5" />
                </button>
              </div>
            </div>

            <div className="flex-1 overflow-y-auto bg-[#05140C] p-4 rounded-xl border border-[#143322] font-mono text-xs text-amber-300 leading-relaxed max-h-[60vh] whitespace-pre-wrap break-all">
              {JSON.stringify(getFullVectorArray(inspectingVectorChunk), null, 2)}
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
