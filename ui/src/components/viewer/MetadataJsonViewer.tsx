import React, { useState } from 'react';
import { Copy, Check } from 'lucide-react';

interface MetadataJsonViewerProps {
  metadata: Record<string, any>;
}

export const MetadataJsonViewer: React.FC<MetadataJsonViewerProps> = ({ metadata }) => {
  const [copied, setCopied] = useState(false);

  const jsonString = JSON.stringify(metadata ?? {}, null, 2);

  const handleCopy = () => {
    navigator.clipboard.writeText(jsonString);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="rounded-2xl bg-black/80 border border-gray-800 overflow-hidden">
      <div className="flex items-center justify-between px-4 py-2.5 bg-gray-900/80 border-b border-gray-800 text-xs text-gray-400">
        <span className="font-semibold text-gray-200">PostgreSQL Metadata JSON Tree</span>
        <button
          onClick={handleCopy}
          className="flex items-center gap-1.5 px-2.5 py-1 rounded bg-gray-800 hover:bg-gray-700 text-gray-300 text-[11px] transition"
        >
          {copied ? (
            <>
              <Check className="w-3 h-3 text-emerald-400" />
              <span className="text-emerald-400">Copied!</span>
            </>
          ) : (
            <>
              <Copy className="w-3 h-3 text-gray-400" />
              <span>Copy JSON</span>
            </>
          )}
        </button>
      </div>

      <pre className="p-6 text-xs font-mono text-amber-300 overflow-x-auto whitespace-pre-wrap leading-relaxed max-h-[60vh]">
        {jsonString}
      </pre>
    </div>
  );
};
