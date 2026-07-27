import { describe, expect, it, beforeEach, afterEach } from 'vitest';
import { render, cleanup } from '@testing-library/react';
import { __resetHotkeys, useHotkeys, useHotkeyLayer } from './hotkeys';

function press(key: string, opts: KeyboardEventInit = {}, target?: HTMLElement) {
  const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...opts });
  (target ?? window).dispatchEvent(event);
  return event;
}

function Binder({
  keys,
  scope,
  onFire,
  allowInInput,
  worksInOverlay,
}: {
  keys: string;
  scope?: 'global' | 'route' | 'sheet' | 'dialog';
  onFire: () => void;
  allowInInput?: boolean;
  worksInOverlay?: boolean;
}) {
  useHotkeys([
    { keys, description: keys, group: 'Test', scope, allowInInput, worksInOverlay, handler: onFire },
  ]);
  return null;
}

function Layer({ scope }: { scope: 'sheet' | 'dialog' | 'palette' }) {
  useHotkeyLayer(scope, true);
  return null;
}

describe('hotkeys', () => {
  beforeEach(() => __resetHotkeys());
  afterEach(() => cleanup());

  it('fires a plain binding', () => {
    let fired = 0;
    render(<Binder keys="j" onFire={() => fired++} />);
    press('j');
    expect(fired).toBe(1);
  });

  it('does not fire in inputs unless allowed', () => {
    let fired = 0;
    render(
      <div>
        <Binder keys="j" onFire={() => fired++} />
        <input data-testid="field" />
      </div>,
    );
    const input = document.querySelector('input')!;
    input.focus();
    press('j', {}, input);
    expect(fired).toBe(0);
  });

  it('fires mod+k inside inputs when allowInInput', () => {
    let fired = 0;
    render(
      <div>
        <Binder keys="mod+k" allowInInput onFire={() => fired++} />
        <input />
      </div>,
    );
    const input = document.querySelector('input')!;
    input.focus();
    const mac = /mac/i.test(navigator.platform);
    press('k', mac ? { metaKey: true } : { ctrlKey: true }, input);
    expect(fired).toBe(1);
  });

  it('completes two-key sequences', () => {
    let fired = 0;
    render(<Binder keys="g p" onFire={() => fired++} />);
    press('g');
    press('p');
    expect(fired).toBe(1);
  });

  it('mutes route/global bindings while an overlay layer is occupied', () => {
    let fired = 0;
    render(
      <div>
        <Binder keys="j" scope="route" onFire={() => fired++} />
        <Layer scope="sheet" />
      </div>,
    );
    press('j');
    expect(fired).toBe(0);
  });

  it('lets worksInOverlay bindings cut through', () => {
    let fired = 0;
    render(
      <div>
        <Binder keys="mod+k" worksInOverlay onFire={() => fired++} />
        <Layer scope="dialog" />
      </div>,
    );
    const mac = /mac/i.test(navigator.platform);
    press('k', mac ? { metaKey: true } : { ctrlKey: true });
    expect(fired).toBe(1);
  });

  it('offers Escape only to the topmost occupied layer', () => {
    let routeEsc = 0;
    let sheetEsc = 0;
    render(
      <div>
        <Binder keys="escape" scope="route" onFire={() => routeEsc++} />
        <Binder keys="escape" scope="sheet" onFire={() => sheetEsc++} />
      </div>,
    );
    press('Escape');
    expect(sheetEsc).toBe(1);
    expect(routeEsc).toBe(0);
  });

  it('a higher layer shadows the same combo below it', () => {
    let route = 0;
    let sheet = 0;
    render(
      <div>
        <Binder keys="d" scope="route" onFire={() => route++} />
        <Binder keys="d" scope="sheet" onFire={() => sheet++} />
      </div>,
    );
    press('d');
    expect(sheet).toBe(1);
    expect(route).toBe(0);
  });
});
