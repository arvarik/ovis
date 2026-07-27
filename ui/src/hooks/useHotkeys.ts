import { useEffect } from 'react';

interface HotkeyOptions {
  onCommandPalette?: () => void;
  onSearchFocus?: () => void;
  onEscape?: () => void;
}

export function useHotkeys({ onCommandPalette, onSearchFocus, onEscape }: HotkeyOptions) {
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Cmd+K or Ctrl+K for Command Palette
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        onCommandPalette?.();
      }

      // '/' to focus search input (if not already inside an input or textarea)
      if (
        e.key === '/' &&
        document.activeElement?.tagName !== 'INPUT' &&
        document.activeElement?.tagName !== 'TEXTAREA'
      ) {
        e.preventDefault();
        onSearchFocus?.();
      }

      // Escape to close modals or drawers
      if (e.key === 'Escape') {
        onEscape?.();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [onCommandPalette, onSearchFocus, onEscape]);
}
