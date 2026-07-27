import React from 'react';
import { Command, RefreshCw } from 'lucide-react';

interface HeaderProps {
  onOpenCommandPalette: () => void;
  onRefresh: () => void;
}

export const Header: React.FC<HeaderProps> = ({
  onOpenCommandPalette,
  onRefresh,
}) => {
  return (
    <header className="h-14 border-b border-[#143322] bg-[#05140C]/95 backdrop-blur-md px-6 flex items-center justify-between sticky top-0 z-30">
      {/* Brand Logo & Name */}
      <div className="flex items-center gap-3">
        <div className="w-8 h-8 rounded-xl bg-gradient-to-tr from-emerald-500 via-teal-500 to-amber-400 flex items-center justify-center font-bold text-gray-950 shadow-lg shadow-emerald-500/20">
          O
        </div>
        <div>
          <div className="flex items-center gap-2">
            <span className="font-bold text-sm tracking-wide text-emerald-100">OVIS</span>
          </div>
          <span className="text-[10px] text-emerald-400/80 block -mt-0.5">Onyx Visibility & Inspection</span>
        </div>
      </div>

      {/* Quick Search Command Palette Trigger */}
      <button
        onClick={onOpenCommandPalette}
        className="hidden md:flex items-center gap-3 px-4 py-1.5 rounded-full bg-[#0A1F13] border border-[#1A422D] hover:border-emerald-500/50 text-xs text-emerald-300/80 hover:text-emerald-100 transition shadow-inner"
      >
        <Command className="w-3.5 h-3.5 text-amber-400" />
        <span>Quick Switcher & Commands...</span>
        <kbd className="px-1.5 py-0.5 rounded bg-[#05140C] text-[10px] font-mono border border-[#1E4D34] text-amber-300">
          ⌘K
        </kbd>
      </button>

      {/* Header Actions */}
      <div className="flex items-center gap-4">
        <button
          onClick={onRefresh}
          className="p-1.5 rounded-lg text-gray-400 hover:text-gray-200 hover:bg-gray-800 transition"
          title="Refresh Data"
        >
          <RefreshCw className="w-4 h-4" />
        </button>
      </div>
    </header>
  );
};
