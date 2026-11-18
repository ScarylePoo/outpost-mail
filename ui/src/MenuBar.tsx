// A plain menu bar. Every entry calls the same function its toolbar button
// or shortcut does.

import { createSignal, For, onCleanup, onMount, Show } from "solid-js";

export interface MenuEntry {
  label: string;
  /** The shortcut, shown at the right. */
  keys?: string;
  run: () => void;
  disabled?: boolean;
  /** Shown with a tick. */
  checked?: boolean;
}

export interface Menu {
  title: string;
  /** `null` is a dividing line. */
  entries: (MenuEntry | null)[];
}

export function MenuBar(props: { menus: Menu[] }) {
  const [open, setOpen] = createSignal<number>();
  let bar!: HTMLElement;

  onMount(() => {
    const away = (e: Event) => {
      if (open() !== undefined && !bar.contains(e.target as Node)) setOpen(undefined);
    };
    const key = (e: KeyboardEvent) => {
      if (open() === undefined) return;
      if (e.key === "Escape") setOpen(undefined);
      else if (e.key === "ArrowRight") setOpen((open()! + 1) % props.menus.length);
      else if (e.key === "ArrowLeft") setOpen((open()! + props.menus.length - 1) % props.menus.length);
      else return;
      e.stopPropagation();
      e.preventDefault();
    };
    window.addEventListener("mousedown", away, true);
    window.addEventListener("blur", () => setOpen(undefined));
    window.addEventListener("keydown", key, true);
    onCleanup(() => {
      window.removeEventListener("mousedown", away, true);
      window.removeEventListener("keydown", key, true);
    });
  });

  return (
    <nav class="menubar" role="menubar" ref={bar}>
      <For each={props.menus}>
        {(menu, index) => (
          <div class="menu">
            <button
              class="menu-title"
              classList={{ open: open() === index() }}
              role="menuitem"
              aria-haspopup="true"
              aria-expanded={open() === index()}
              onMouseDown={(e) => {
                e.preventDefault();
                setOpen(open() === index() ? undefined : index());
              }}
              onMouseEnter={() => open() !== undefined && setOpen(index())}
            >
              {menu.title}
            </button>
            <Show when={open() === index()}>
              <div class="menu-list" role="menu">
                <For each={menu.entries}>
                  {(entry) => (
                    <Show when={entry} fallback={<div class="menu-line" role="separator" />}>
                      {(e) => (
                        <button
                          role="menuitem"
                          disabled={e().disabled}
                          onClick={() => {
                            setOpen(undefined);
                            e().run();
                          }}
                        >
                          <span class="menu-tick">{e().checked ? "✓" : ""}</span>
                          <span class="menu-label">{e().label}</span>
                          <span class="menu-keys">{e().keys ?? ""}</span>
                        </button>
                      )}
                    </Show>
                  )}
                </For>
              </div>
            </Show>
          </div>
        )}
      </For>
    </nav>
  );
}
