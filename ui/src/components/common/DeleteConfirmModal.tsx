import React from 'react';
import { AlertTriangle, Trash2 } from 'lucide-react';
import { Modal } from './Modal';
import { PageListItem } from '../../api/types';

interface DeleteConfirmModalProps {
  isOpen: boolean;
  onClose: () => void;
  onConfirm: () => void;
  targetPage?: PageListItem | null;
  targetBatchIds?: string[];
}

export const DeleteConfirmModal: React.FC<DeleteConfirmModalProps> = ({
  isOpen,
  onClose,
  onConfirm,
  targetPage,
  targetBatchIds = [],
}) => {
  const isBatch = targetBatchIds.length > 0;
  const count = isBatch ? targetBatchIds.length : 1;

  return (
    <Modal isOpen={isOpen} onClose={onClose} maxWidth="md">
      <div className="space-y-4">
        {/* Header Warning */}
        <div className="flex items-center gap-3 border-b border-[#143322] pb-3">
          <div className="p-2 rounded-xl bg-rose-500/20 text-rose-400 border border-rose-500/40">
            <AlertTriangle className="w-5 h-5" />
          </div>
          <div>
            <h3 className="text-sm font-bold text-rose-200">
              Confirm Document Purge ({count} {count === 1 ? 'item' : 'items'})
            </h3>
            <p className="text-xs text-emerald-400/70">
              This action will remove document records from SQL & OpenSearch vector index.
            </p>
          </div>
        </div>

        {/* Target Details Box */}
        <div className="p-3.5 rounded-xl bg-[#05140C] border border-[#173826] space-y-2 text-xs font-mono">
          {isBatch ? (
            <div>
              <span className="text-amber-300 font-semibold">Batch Purge Selection:</span>
              <p className="text-gray-300 mt-1 max-h-24 overflow-y-auto break-all">
                {targetBatchIds.join(', ')}
              </p>
            </div>
          ) : targetPage ? (
            <div className="space-y-1">
              <div className="flex justify-between text-gray-300">
                <span className="text-emerald-400/80">Title:</span>
                <span className="text-amber-300 font-semibold truncate max-w-[240px]">
                  {targetPage.semantic_id || targetPage.id}
                </span>
              </div>
              <div className="flex justify-between text-gray-300">
                <span className="text-emerald-400/80">Document ID:</span>
                <span className="text-emerald-200 truncate max-w-[240px]">{targetPage.id}</span>
              </div>
              {targetPage.link && (
                <div className="flex justify-between text-gray-300">
                  <span className="text-emerald-400/80">URL:</span>
                  <span className="text-indigo-300 truncate max-w-[240px]">{targetPage.link}</span>
                </div>
              )}
            </div>
          ) : null}
        </div>

        {/* Action Buttons */}
        <div className="flex items-center justify-end gap-3 pt-2">
          <button
            onClick={onClose}
            className="px-4 py-2 rounded-xl bg-[#05140C] hover:bg-[#112A1B] text-emerald-200 border border-[#173826] text-xs font-medium transition"
          >
            Cancel
          </button>
          <button
            onClick={() => {
              onConfirm();
              onClose();
            }}
            className="px-4 py-2 rounded-xl bg-rose-600 hover:bg-rose-500 text-white text-xs font-semibold flex items-center gap-1.5 shadow-lg shadow-rose-950/50 transition"
          >
            <Trash2 className="w-3.5 h-3.5" />
            <span>Purge & Delete ({count})</span>
          </button>
        </div>
      </div>
    </Modal>
  );
};
