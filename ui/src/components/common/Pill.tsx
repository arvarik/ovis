import React from 'react';
import { clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';

interface PillProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  active?: boolean;
  variant?: 'rose' | 'violet' | 'emerald' | 'amber' | 'slate';
  size?: 'sm' | 'md';
  icon?: React.ReactNode;
}

export const Pill: React.FC<PillProps> = ({
  children,
  active = false,
  variant = 'rose',
  size = 'md',
  icon,
  className,
  ...props
}) => {
  const baseStyle =
    'inline-flex items-center gap-1.5 rounded-full font-medium transition-all duration-200 focus:outline-none focus:ring-2 focus:ring-rose-500/50 cursor-pointer select-none';

  const sizeStyle = size === 'sm' ? 'px-2.5 py-1 text-xs' : 'px-3.5 py-1.5 text-xs';

  const variantStyles = {
    rose: active
      ? 'bg-rose-500 text-white shadow-lg shadow-rose-500/30 border border-rose-400'
      : 'bg-gray-800/80 text-gray-300 hover:bg-gray-700/80 border border-gray-700/60 hover:border-gray-600',
    violet: active
      ? 'bg-violet-600 text-white shadow-lg shadow-violet-600/30 border border-violet-500'
      : 'bg-gray-800/80 text-gray-300 hover:bg-gray-700/80 border border-gray-700/60 hover:border-gray-600',
    emerald: active
      ? 'bg-emerald-600 text-white shadow-lg shadow-emerald-600/30 border border-emerald-500'
      : 'bg-gray-800/80 text-gray-300 hover:bg-gray-700/80 border border-gray-700/60 hover:border-gray-600',
    amber: active
      ? 'bg-amber-500 text-white shadow-lg shadow-amber-500/30 border border-amber-400'
      : 'bg-gray-800/80 text-gray-300 hover:bg-gray-700/80 border border-gray-700/60 hover:border-gray-600',
    slate: active
      ? 'bg-gray-700 text-white border border-gray-600'
      : 'bg-gray-800/60 text-gray-400 hover:bg-gray-800 hover:text-gray-200 border border-gray-800',
  };

  return (
    <button
      className={twMerge(clsx(baseStyle, sizeStyle, variantStyles[variant], className))}
      {...props}
    >
      {icon && <span className="w-3.5 h-3.5">{icon}</span>}
      <span>{children}</span>
    </button>
  );
};
