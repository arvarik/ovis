import React from 'react';

interface RawTextViewerProps {
  fullText: string;
}

export const RawTextViewer: React.FC<RawTextViewerProps> = ({ fullText }) => {
  const lines = fullText ? fullText.split('\n') : [''];

  return (
    <div className="rounded-2xl bg-black/80 border border-gray-800 overflow-hidden font-mono text-xs text-emerald-400">
      <div className="flex items-center justify-between px-4 py-2 bg-gray-900/80 border-b border-gray-800 text-[11px] text-gray-400 select-none">
        <span>Raw Text Buffer ({lines.length} lines)</span>
        <span>UTF-8</span>
      </div>

      <div className="p-4 overflow-x-auto max-h-[60vh] space-y-1">
        {lines.map((line, idx) => (
          <div key={idx} className="flex gap-4 hover:bg-gray-900/50 px-1 rounded transition">
            <span className="w-10 shrink-0 text-right text-gray-600 select-none font-mono">
              {idx + 1}
            </span>
            <span className="flex-1 whitespace-pre-wrap text-gray-200">{line}</span>
          </div>
        ))}
      </div>
    </div>
  );
};
