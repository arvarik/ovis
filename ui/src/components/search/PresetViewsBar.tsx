import React from 'react';
import { Layers, AlertTriangle, Zap, Globe, Link2, Clock } from 'lucide-react';

export type PresetViewId = 'all' | 'stubs' | 'heavy' | 'web' | 'links' | 'recent';

interface PresetViewsBarProps {
  activePreset: PresetViewId;
  onSelectPreset: (preset: PresetViewId) => void;
  counts: {
    all: number;
    stubs: number;
    heavy: number;
    web: number;
    links: number;
  };
}

export const PresetViewsBar: React.FC<PresetViewsBarProps> = ({
  activePreset,
  onSelectPreset,
  counts,
}) => {
  const presets: { id: PresetViewId; label: string; icon: any; count?: number; color: string }[] = [
    { id: 'all', label: 'All Pages', icon: Layers, count: counts.all, color: 'text-amber-400' },
    { id: 'stubs', label: 'Empty Stubs', icon: AlertTriangle, count: counts.stubs, color: 'text-rose-400' },
    { id: 'heavy', label: 'Heavy Vector Docs (>10 Chunks)', icon: Zap, count: counts.heavy, color: 'text-emerald-400' },
    { id: 'web', label: 'Web Crawls', icon: Globe, count: counts.web, color: 'text-indigo-400' },
    { id: 'links', label: 'With External Links', icon: Link2, count: counts.links, color: 'text-teal-400' },
    { id: 'recent', label: 'Recently Updated', icon: Clock, color: 'text-violet-400' },
  ];

  return (
    <div className="flex items-center gap-1.5 overflow-x-auto pb-1 select-none scrollbar-none">
      <span className="text-[10px] font-semibold uppercase tracking-wider text-emerald-500/80 mr-1 shrink-0">
        Saved Views:
      </span>
      {presets.map((preset) => {
        const Icon = preset.icon;
        const isActive = activePreset === preset.id;
        return (
          <button
            key={preset.id}
            onClick={() => onSelectPreset(preset.id)}
            className={`flex items-center gap-1.5 px-3 py-1 rounded-full text-xs font-medium border transition shrink-0 ${
              isActive
                ? 'bg-amber-500/20 text-amber-300 border-amber-500/50 font-semibold shadow-sm'
                : 'bg-[#0A1F13]/80 text-emerald-100/80 hover:text-white hover:bg-[#112A1B] border-[#143322]'
            }`}
          >
            <Icon className={`w-3.5 h-3.5 ${preset.color}`} />
            <span>{preset.label}</span>
            {preset.count !== undefined && (
              <span
                className={`px-1.5 py-0.2 rounded-full text-[10px] font-mono ${
                  isActive ? 'bg-amber-950 text-amber-200 border border-amber-600/50' : 'bg-[#05140C] text-emerald-300'
                }`}
              >
                {preset.count.toLocaleString()}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
};
