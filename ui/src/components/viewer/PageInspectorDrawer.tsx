import React, { useState, useEffect } from 'react';
import { X, FileText, Code, Layers, FileCode, Trash2, ExternalLink, RefreshCw } from 'lucide-react';
import { PageDetailResponse, PageListItem } from '../../api/types';
import { fetchPageDetail } from '../../api/client';
import { MarkdownViewer } from './MarkdownViewer';
import { RawTextViewer } from './RawTextViewer';
import { ChunkMapViewer } from './ChunkMapViewer';
import { MetadataJsonViewer } from './MetadataJsonViewer';

interface PageInspectorDrawerProps {
  isOpen: boolean;
  onClose: () => void;
  selectedPage: PageListItem | null;
  onDeletePage: (id: string) => void;
}

export const PageInspectorDrawer: React.FC<PageInspectorDrawerProps> = ({
  isOpen,
  onClose,
  selectedPage,
  onDeletePage,
}) => {
  const [activeTab, setActiveTab] = useState<'markdown' | 'raw' | 'chunks' | 'metadata'>('markdown');
  const [detailData, setDetailData] = useState<PageDetailResponse | null>(null);
  const [loading, setLoading] = useState<boolean>(false);

  useEffect(() => {
    if (isOpen && selectedPage) {
      setLoading(true);
      fetchPageDetail(selectedPage.id)
        .then((data) => {
          setDetailData(data);
        })
        .catch((err) => {
          console.error('Failed to load page detail:', err);
          setDetailData(null);
        })
        .finally(() => setLoading(false));
    }
  }, [isOpen, selectedPage]);

  if (!isOpen || !selectedPage) return null;

  const currentConnector = selectedPage.connector_name || selectedPage.connector_source || 'Connector';

  return (
    <div
      className="fixed inset-0 z-50 overflow-hidden bg-black/75 backdrop-blur-sm transition-opacity flex justify-end cursor-pointer"
      onClick={onClose}
    >
      <div
        className="w-screen max-w-3xl bg-[#05140C] border-l border-[#143322] shadow-2xl flex flex-col h-full animate-slideInRight cursor-default"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header Bar with Breadcrumb Navigation */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-[#143322] bg-[#0A1F13]/90">
          <div className="flex items-center gap-2 text-xs text-emerald-400/80 font-mono min-w-0">
            <span>Connectors</span>
            <span>/</span>
            <span className="text-amber-300 font-semibold shrink-0">{currentConnector}</span>
            <span>/</span>
            <span className="text-emerald-100 truncate max-w-[200px]">{selectedPage.semantic_id}</span>
          </div>

          <div className="flex items-center gap-2 shrink-0">
            <button
              onClick={() => onDeletePage(selectedPage.id)}
              className="px-3 py-1.5 rounded-lg bg-rose-500/10 hover:bg-rose-500/20 text-rose-400 text-xs font-semibold flex items-center gap-1.5 border border-rose-500/30 transition"
            >
              <Trash2 className="w-3.5 h-3.5" />
              <span>Delete Page</span>
            </button>

            <button
              onClick={onClose}
              className="p-1.5 rounded-lg text-gray-400 hover:text-gray-100 hover:bg-gray-800 transition"
              title="Close Drawer (Esc or Click Outside)"
            >
              <X className="w-5 h-5" />
            </button>
          </div>
        </div>

        {/* Title & Document Canvas Header Metadata Card */}
        <div className="px-8 py-5 border-b border-[#143322] bg-[#0A1F13]/60 space-y-4">
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0 flex-1">
              <h1 className="text-xl font-bold text-gray-100 leading-tight break-words">
                {detailData?.semantic_id || selectedPage.semantic_id || selectedPage.id}
              </h1>

              {(detailData?.link || selectedPage.link) && (
                <a
                  href={detailData?.link || selectedPage.link}
                  target="_blank"
                  rel="noreferrer"
                  className="text-xs text-indigo-400 hover:underline inline-flex items-center gap-1 font-mono truncate max-w-lg mt-1"
                >
                  <span className="truncate">{detailData?.link || selectedPage.link}</span>
                  <ExternalLink className="w-3 h-3 shrink-0" />
                </a>
              )}
            </div>

            <div
              className="shrink-0 font-mono text-[11px] px-2.5 py-1 rounded-md bg-gray-900 border border-gray-800 text-gray-400 max-w-[200px] truncate"
              title={`Document ID: ${selectedPage.id}`}
            >
              ID: <span className="text-amber-300 font-semibold truncate">{selectedPage.id}</span>
            </div>
          </div>

          {/* Owners and Connector Badges */}
          <div className="flex flex-wrap items-center gap-2 text-xs">
            {/* Connector Badge */}
            <span className="inline-flex items-center gap-1 px-2.5 py-1 rounded-full bg-violet-600/20 border border-violet-500/40 text-violet-300 text-[11px]">
              <span>Connector: {detailData?.connector_name || selectedPage.connector_name || selectedPage.connector_source || 'Unknown'}</span>
              {(detailData?.connector_id || selectedPage.connector_id) && (
                <span className="font-mono text-[10px] bg-violet-950 px-1.5 py-0.2 rounded text-violet-200">
                  #{detailData?.connector_id || selectedPage.connector_id}
                </span>
              )}
            </span>

            {/* Primary Owners */}
            {detailData?.primary_owners && detailData.primary_owners.length > 0 && (
              <div className="flex items-center gap-1">
                <span className="text-[10px] text-gray-400 font-medium">Owners:</span>
                {detailData.primary_owners.map((owner) => (
                  <span
                    key={owner}
                    className="inline-flex items-center gap-1 px-2.5 py-0.5 rounded-full bg-emerald-500/20 text-emerald-300 border border-emerald-500/40 text-[11px] font-mono"
                  >
                    <span>{owner}</span>
                  </span>
                ))}
              </div>
            )}

            {/* Secondary Owners */}
            {detailData?.secondary_owners && detailData.secondary_owners.length > 0 && (
              <div className="flex items-center gap-1">
                {detailData.secondary_owners.map((owner) => (
                  <span
                    key={owner}
                    className="inline-flex items-center gap-1 px-2.5 py-0.5 rounded-full bg-indigo-500/20 text-indigo-300 border border-indigo-500/40 text-[11px] font-mono"
                  >
                    <span>{owner}</span>
                  </span>
                ))}
              </div>
            )}
          </div>

          {/* Extracted Key-Value Metadata Tags */}
          {((detailData?.metadata && Object.keys(detailData.metadata).length > 0) ||
            (selectedPage.metadata && Object.keys(selectedPage.metadata).length > 0)) && (
            <div className="flex flex-wrap items-center gap-1.5 pt-1">
              <span className="text-[10px] uppercase tracking-wider font-semibold text-gray-500 mr-1">Tags:</span>
              {Object.entries(detailData?.metadata || selectedPage.metadata).map(([key, val]) => {
                if (typeof val === 'object' && val !== null) return null;
                return (
                  <span
                    key={key}
                    className="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-gray-900/90 text-gray-300 border border-gray-800 text-[10px] font-mono"
                  >
                    <span className="text-gray-400 font-semibold">{key}:</span>
                    <span className="text-amber-300">{String(val)}</span>
                  </span>
                );
              })}
            </div>
          )}
        </div>

        {/* Tab Selection Row */}
        <div className="flex items-center gap-1 px-8 border-b border-gray-800 bg-[#0F172A]/40 shrink-0">
          {[
            { id: 'markdown', label: 'Rendered View', icon: FileText },
            { id: 'raw', label: 'Raw Text', icon: Code },
            { id: 'chunks', label: `Chunks (${detailData?.chunks?.length ?? selectedPage.chunk_count})`, icon: Layers },
            { id: 'metadata', label: 'Metadata JSON', icon: FileCode },
          ].map((tab) => {
            const Icon = tab.icon;
            const isActive = activeTab === tab.id;
            return (
              <button
                key={tab.id}
                onClick={() => setActiveTab(tab.id as any)}
                className={`flex items-center gap-2 px-4 py-3 text-xs font-semibold border-b-2 transition ${
                  isActive
                    ? 'border-rose-500 text-rose-400 bg-rose-500/5'
                    : 'border-transparent text-gray-400 hover:text-gray-200'
                }`}
              >
                <Icon className="w-3.5 h-3.5" />
                <span>{tab.label}</span>
              </button>
            );
          })}
        </div>

        {/* Content Canvas */}
        <div className="flex-1 overflow-y-auto p-8">
          {loading ? (
            <div className="flex flex-col items-center justify-center h-64 space-y-3 text-gray-400 text-xs">
              <RefreshCw className="w-6 h-6 animate-spin text-rose-400" />
              <span>Loading document inspector details...</span>
            </div>
          ) : !detailData ? (
            <div className="text-center text-gray-400 text-xs py-10">Unable to load document details.</div>
          ) : (
            <>
              {activeTab === 'markdown' && (
                <MarkdownViewer
                  fullText={detailData.full_text}
                  chunkCount={detailData.chunks.length}
                />
              )}

              {activeTab === 'raw' && <RawTextViewer fullText={detailData.full_text} />}

              {activeTab === 'chunks' && <ChunkMapViewer chunks={detailData.chunks} />}

              {activeTab === 'metadata' && (
                <MetadataJsonViewer
                  metadata={{
                    document: {
                      id: detailData.id,
                      semantic_id: detailData.semantic_id,
                      link: detailData.link,
                      doc_updated_at: detailData.doc_updated_at || selectedPage.doc_updated_at,
                      primary_owners: detailData.primary_owners || [],
                      secondary_owners: detailData.secondary_owners || [],
                    },
                    connector: {
                      id: detailData.connector_id || selectedPage.connector_id,
                      name: detailData.connector_name || selectedPage.connector_name,
                      source: detailData.connector_source || selectedPage.connector_source,
                    },
                    opensearch_index: {
                      target_index: 'danswer_chunk',
                      total_chunks: detailData.chunks.length,
                      embedding_model: '1536d-nomic-embed-text / OpenAI text-embedding-3-small',
                      chunk_boundaries: detailData.chunks.map((c) => ({
                        chunk_id: c.chunk_id,
                        token_count: c.token_count,
                        content_preview: c.content.length > 80 ? c.content.substring(0, 80) + '...' : c.content,
                      })),
                    },
                    custom_metadata: detailData.metadata || selectedPage.metadata || {},
                  }}
                />
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
};
