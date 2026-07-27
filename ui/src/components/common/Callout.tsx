import React from 'react';
import { clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';

interface CalloutProps {
  icon?: string | React.ReactNode;
  title?: string;
  children: React.ReactNode;
  variant?: 'info' | 'warning' | 'success' | 'danger';
  className?: string;
}

export const Callout: React.FC<CalloutProps> = ({
  icon = '💡',
  title,
  children,
  variant = 'info',
  className,
}) => {
  const variantStyles = {
    info: 'bg-gray-900/90 border-gray-800 text-gray-300',
    warning: 'bg-amber-950/30 border-amber-800/60 text-amber-200',
    success: 'bg-emerald-950/30 border-emerald-800/60 text-emerald-200',
    danger: 'bg-rose-950/30 border-rose-800/60 text-rose-200',
  };

  return (
    <div
      className={twMerge(
        clsx(
          'p-4 rounded-xl border text-xs flex items-start gap-3 shadow-md backdrop-blur-sm',
          variantStyles[variant],
          className
        )
      )}
    >
      <div className="text-base select-none shrink-0 mt-0.5">{icon}</div>
      <div className="flex-1 space-y-1">
        {title && <h4 className="font-semibold text-gray-100">{title}</h4>}
        <div className="leading-relaxed text-gray-300">{children}</div>
      </div>
    </div>
  );
};
