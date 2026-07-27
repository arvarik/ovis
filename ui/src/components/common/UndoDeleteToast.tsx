import React, { useEffect, useState } from 'react';
import { RotateCcw, X, Trash2 } from 'lucide-react';
import { PageListItem } from '../../api/types';

export interface UndoableDeleteBatch {
  id: string;
  pages: PageListItem[];
  timestamp: number;
}

interface UndoDeleteToastProps {
  batch: UndoableDeleteBatch | null;
  onUndo: (batch: UndoableDeleteBatch) => void;
  onDismiss: () => void;
}

export const UndoDeleteToast: React.FC<UndoDeleteToastProps> = ({ batch, onUndo, onDismiss }) => {
  const [timeLeftSec, setTimeLeftSec] = useState<number>(5);

  useEffect(() => {
    if (!batch) return;

    setTimeLeftSec(5);
    const interval = setInterval(() => {
      setTimeLeftSec((prev) => {
        if (prev <= 1) {
          clearInterval(interval);
          onDismiss();
          return 0;
        }
        return prev - 1;
      });
    }, 1000);

    return () => clearInterval(interval);
  }, [batch, onDismiss]);

  if (!batch) return null;

  const count = batch.pages.length;
  const label = count === 1 ? `1 document deleted` : `${count} documents deleted`;
  const progressPercent = (timeLeftSec / 5) * 100;

  return (
    <div className="fixed bottom-6 right-6 z-50 flex flex-col pointer-events-auto max-w-sm w-full animate-fadeIn">
      <div className="rounded-2xl bg-[#0A1F13] border border-rose-800/80 p-4 shadow-2xl backdrop-blur-2xl space-y-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2.5">
            <div className="p-1.5 rounded-lg bg-rose-500/20 text-rose-400 border border-rose-500/40">
              <Trash2 className="w-4 h-4" />
            </div>
            <div>
              <p className="text-xs font-bold text-rose-200">{label}</p>
              <p className="text-[11px] text-emerald-400/80 font-mono mt-0.5">
                Undo window closes in {timeLeftSec}s
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2">
            <button
              onClick={() => onUndo(batch)}
              className="flex items-center gap-1.5 px-3 py-1.5 rounded-xl bg-amber-500/20 hover:bg-amber-500/30 text-amber-300 border border-amber-500/50 text-xs font-bold shadow-sm transition"
              title="Click to restore deleted documents"
            >
              <RotateCcw className="w-3.5 h-3.5 text-amber-400" />
              <span>Undo</span>
            </button>

            <button
              onClick={onDismiss}
              className="p-1 rounded-lg text-gray-400 hover:text-white hover:bg-[#112A1B] transition"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>

        {/* 5-second countdown progress bar */}
        <div className="w-full bg-[#05140C] h-1 rounded-full overflow-hidden border border-[#173826]">
          <div
            className="bg-amber-400 h-full transition-all duration-1000 ease-linear"
            style={{ width: `${progressPercent}%` }}
          />
        </div>
      </div>
    </div>
  );
};
