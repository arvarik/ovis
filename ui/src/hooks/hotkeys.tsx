/**
 * Layered keyboard shortcut system.
 *
 * One registry drives both dispatch and the `?` help overlay, so bindings and
 * their documentation cannot drift. Scopes are layered with priority
 * dialog > sheet > palette > route > global:
 *
 * - For most keys, layers are walked top-down and the first match wins
 *   (a higher layer shadows the same combo below it).
 * - `Escape` is offered ONLY to the topmost occupied layer — one layer at a
 *   time, fixing the old UI's "Esc closes drawer+palette+modal at once".
 * - Radix/vaul overlays handle their own Escape; they occupy a layer via
 *   `useHotkeyLayer` so route/global Escape bindings stay quiet while any
 *   overlay is open. Events they consume arrive here `defaultPrevented` and
 *   are ignored.
 *
 * Sequences ("g p") are supported with an 800 ms window; typing targets
 * (inputs, textareas, contenteditable) only receive bindings that opt in via
 * `allowInInput`.
 */
import { useEffect, useRef, useSyncExternalStore } from 'react';

export type HotkeyScope = 'global' | 'route' | 'palette' | 'sheet' | 'dialog';

/** Highest priority first. */
const PRIORITY: readonly HotkeyScope[] = ['dialog', 'sheet', 'palette', 'route', 'global'];

export interface HotkeyBinding {
  /** 'mod+k', '/', 'g p' (sequence), 'shift+x', '[', '?', 'escape' */
  keys: string;
  /** Rendered in the ? help overlay. */
  description: string;
  /** Help overlay group heading, e.g. 'Navigation'. */
  group: string;
  scope?: HotkeyScope;
  /** Fire even when an input/textarea/contenteditable has focus. */
  allowInInput?: boolean;
  /**
   * Route/global bindings are muted while any overlay layer (dialog, sheet,
   * palette) is occupied — pressing `j` with the inspector open must not move
   * the list behind it. Set this for the few that should cut through (⌘K).
   */
  worksInOverlay?: boolean;
  /** Hidden from the help overlay (layer markers, internal bindings). */
  hidden?: boolean;
  handler: (e: KeyboardEvent) => void;
}

interface Registered extends HotkeyBinding {
  id: number;
  scope: HotkeyScope;
}

const SEQUENCE_WINDOW_MS = 800;
const IS_MAC = typeof navigator !== 'undefined' && /mac/i.test(navigator.platform);

let nextId = 1;
let bindings: Registered[] = [];
const listeners = new Set<() => void>();

function notify() {
  for (const l of listeners) l();
}

function register(binding: HotkeyBinding): () => void {
  const entry: Registered = { ...binding, scope: binding.scope ?? 'global', id: nextId++ };
  bindings = [...bindings, entry];
  notify();
  return () => {
    bindings = bindings.filter((b) => b.id !== entry.id);
    notify();
  };
}

/** Test hook: wipe the registry between cases. */
export function __resetHotkeys() {
  bindings = [];
  pendingPrefix = null;
  notify();
}

function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT';
}

interface ParsedCombo {
  key: string;
  mod: boolean;
  shift: boolean;
  alt: boolean;
}

function parseCombo(combo: string): ParsedCombo {
  const parts = combo.toLowerCase().split('+');
  const key = parts[parts.length - 1] ?? '';
  return {
    key,
    mod: parts.includes('mod'),
    shift: parts.includes('shift'),
    alt: parts.includes('alt'),
  };
}

function comboMatches(combo: string, e: KeyboardEvent): boolean {
  const parsed = parseCombo(combo);
  const key = e.key.toLowerCase();
  if (parsed.key !== key) return false;
  const modActive = IS_MAC ? e.metaKey : e.ctrlKey;
  if (parsed.mod !== modActive) return false;
  if (parsed.alt !== e.altKey) return false;
  // Shift is only asserted when the combo names it AND the key is a letter —
  // punctuation like '?' already encodes shift in e.key.
  if (parsed.key.length === 1 && /[a-z]/.test(parsed.key) && parsed.shift !== e.shiftKey) {
    return false;
  }
  return true;
}

let pendingPrefix: { key: string; at: number } | null = null;

const OVERLAY_SCOPES: readonly HotkeyScope[] = ['dialog', 'sheet', 'palette'];

function topmostOccupiedScope(): HotkeyScope | null {
  for (const scope of PRIORITY) {
    if (bindings.some((b) => b.scope === scope)) return scope;
  }
  return null;
}

function overlayActive(): boolean {
  return bindings.some((b) => OVERLAY_SCOPES.includes(b.scope));
}

function dispatch(e: KeyboardEvent) {
  if (e.defaultPrevented) return;
  // Bare modifier presses never match and must not clear a pending sequence.
  if (e.key === 'Meta' || e.key === 'Control' || e.key === 'Alt' || e.key === 'Shift') return;

  const inInput = isEditable(e.target);
  const muteLower = overlayActive();
  const candidates = bindings.filter((b) => {
    if (b.keys === '') return false;
    if (inInput && !b.allowInInput) return false;
    if (muteLower && !OVERLAY_SCOPES.includes(b.scope) && !b.worksInOverlay) return false;
    return true;
  });

  // Escape goes to the topmost occupied layer only.
  if (e.key === 'Escape') {
    pendingPrefix = null;
    const top = topmostOccupiedScope();
    if (!top) return;
    const match = candidates.find((b) => b.scope === top && comboMatches(b.keys, e));
    if (match) {
      e.preventDefault();
      match.handler(e);
    }
    return;
  }

  const key = e.key.toLowerCase();

  // Complete a pending sequence ('g' then 'p').
  if (pendingPrefix && Date.now() - pendingPrefix.at < SEQUENCE_WINDOW_MS) {
    const prefix = pendingPrefix.key;
    pendingPrefix = null;
    for (const scope of PRIORITY) {
      const match = candidates.find((b) => {
        if (b.scope !== scope) return false;
        const parts = b.keys.split(' ');
        return parts.length === 2 && parts[0] === prefix && parts[1] === key;
      });
      if (match) {
        e.preventDefault();
        match.handler(e);
        return;
      }
    }
    // fall through: the second key may match a plain binding
  } else {
    pendingPrefix = null;
  }

  // Plain (non-sequence) bindings, top layer first.
  for (const scope of PRIORITY) {
    const match = candidates.find(
      (b) => b.scope === scope && !b.keys.includes(' ') && comboMatches(b.keys, e),
    );
    if (match) {
      e.preventDefault();
      match.handler(e);
      return;
    }
  }

  // Start a sequence if this key opens one.
  const opensSequence = candidates.some((b) => {
    const parts = b.keys.split(' ');
    return parts.length === 2 && parts[0] === key && !e.metaKey && !e.ctrlKey && !e.altKey;
  });
  if (opensSequence) {
    pendingPrefix = { key, at: Date.now() };
    e.preventDefault();
  }
}

let dispatcherInstalled = false;
function ensureDispatcher() {
  if (dispatcherInstalled || typeof window === 'undefined') return;
  dispatcherInstalled = true;
  window.addEventListener('keydown', dispatch);
}

/**
 * Register bindings for the lifetime of the component. Handlers are kept in a
 * ref, so re-renders never re-register; the binding set re-registers only when
 * the printed key list changes or `enabled` flips.
 */
export function useHotkeys(bindingsIn: HotkeyBinding[], enabled = true) {
  const ref = useRef(bindingsIn);
  useEffect(() => {
    ref.current = bindingsIn;
  });
  const signature = bindingsIn.map((b) => `${b.scope ?? 'global'}:${b.keys}`).join('|');

  useEffect(() => {
    if (!enabled) return;
    ensureDispatcher();
    const unregisters = ref.current.map((b, i) =>
      register({
        ...b,
        handler: (e) => ref.current[i]?.handler(e),
      }),
    );
    return () => unregisters.forEach((un) => un());
  }, [signature, enabled]);
}

/**
 * Occupy a layer while an overlay is open, so lower layers stop receiving
 * Escape (and can be shadowed). Radix handles the actual Escape-to-close.
 */
export function useHotkeyLayer(scope: HotkeyScope, active: boolean) {
  useEffect(() => {
    if (!active) return;
    ensureDispatcher();
    return register({
      keys: '',
      description: '',
      group: '',
      scope,
      hidden: true,
      handler: () => {},
    });
  }, [scope, active]);
}

/** Live binding list for the ? help overlay. */
export function useHotkeyList(): Registered[] {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => bindings,
  );
}

/** 'mod+k' -> ['⌘', 'K'] on mac, ['Ctrl', 'K'] elsewhere; 'g p' -> ['G', 'P']. */
export function comboLabel(keys: string): string[] {
  if (keys.includes(' ')) {
    return keys.split(' ').map((k) => k.toUpperCase());
  }
  return keys.split('+').map((part) => {
    switch (part) {
      case 'mod':
        return IS_MAC ? '⌘' : 'Ctrl';
      case 'shift':
        return '⇧';
      case 'alt':
        return IS_MAC ? '⌥' : 'Alt';
      case 'escape':
        return 'Esc';
      case 'enter':
        return '↵';
      case 'arrowup':
        return '↑';
      case 'arrowdown':
        return '↓';
      default:
        return part.length === 1 ? part.toUpperCase() : part;
    }
  });
}
