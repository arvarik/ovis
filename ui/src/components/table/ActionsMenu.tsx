import React from 'react';
import { Eye, Trash2, ExternalLink } from 'lucide-react';

interface ActionsMenuProps {
  onInspect: () => void;
  onDelete: () => void;
  link?: string;
}

export const ActionsMenu: React.FC<ActionsMenuProps> = ({ onInspect, onDelete, link }) => {
  return (
    <div className="flex items-center gap-1 opacity-80 group-hover:opacity-100 transition">
      <button
        onClick={(e) => {
          e.stopPropagation();
          onInspect();
        }}
        className="p-1.5 rounded-lg text-gray-400 hover:text-gray-100 hover:bg-gray-800 transition"
        title="View Page"
        aria-label="View Page"
      >
        <Eye className="w-4 h-4 text-indigo-400" />
      </button>

      {link && (
        <a
          href={link}
          target="_blank"
          rel="noreferrer"
          onClick={(e) => e.stopPropagation()}
          className="p-1.5 rounded-lg text-gray-400 hover:text-gray-100 hover:bg-gray-800 transition"
          title="Open External Link"
        >
          <ExternalLink className="w-4 h-4 text-gray-400" />
        </a>
      )}

      <button
        onClick={(e) => {
          e.stopPropagation();
          onDelete();
        }}
        className="p-1.5 rounded-lg text-gray-400 hover:text-rose-400 hover:bg-rose-500/10 transition"
        title="Delete Page (Cascading SQL & OpenSearch)"
      >
        <Trash2 className="w-4 h-4" />
      </button>
    </div>
  );
};
