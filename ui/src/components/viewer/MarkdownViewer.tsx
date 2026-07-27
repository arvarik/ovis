import React from 'react';
import { Callout } from '../common/Callout';

interface MarkdownViewerProps {
  fullText: string;
  chunkCount: number;
}

export const MarkdownViewer: React.FC<MarkdownViewerProps> = ({ fullText, chunkCount }) => {
  return (
    <div className="space-y-6">
      <Callout icon="💡" title="Document Inspection Notice" variant="info">
        This document contains <strong className="text-gray-100">{chunkCount} chunk(s)</strong> indexed in OpenSearch. You are currently viewing the reconstructed full text document canvas.
      </Callout>

      <div className="prose prose-invert max-w-none text-sm leading-relaxed whitespace-pre-wrap font-sans text-gray-200 bg-gray-900/40 p-6 rounded-2xl border border-gray-800/80">
        {fullText || <span className="text-gray-500 italic">No document text available.</span>}
      </div>
    </div>
  );
};
