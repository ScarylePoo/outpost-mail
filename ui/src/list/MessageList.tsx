// The message list: a virtualised, paged view of one folder.
//
// Only the rows on screen (plus a margin) exist in the DOM. Row elements are
// pooled: a row keeps its element for as long as it is visible, and an element
// that scrolls off is reused for a row that scrolls on. Row data is fetched
// from the core in pages and cached.

import { createEffect, createMemo, createSignal, Index, on, onCleanup, onMount, Show } from "solid-js";
import { core, type FolderId, type MessageRow } from "../core";
import { buildLayout, LINE, ROW_FIXED, rowAt, type Layout } from "./layout";
import { formatDate } from "./format";

const PAGE = 200;
/** Rows kept rendered above and below the visible ones. */
const OVERSCAN = 12;
const MAX_CACHED_PAGES = 80;
/** The tallest scrollable area used. Well inside every engine's limit. */
const MAX_SCROLL_HEIGHT = 8_000_000;

export interface ListHandle {
  scroller: HTMLElement;
  /** Visible rows whose data has not arrived yet. Should be 0. */
  blankVisible(): number;
  rowCount(): number;
  /** First and last row positions currently in view. */
  visibleRange(): [number, number];
  /** The selected message, if there is one and its data is loaded. */
  selectedRow(): MessageRow | undefined;
  /** Selects the row at this position and scrolls it into view. */
  select(index: number): void;
  /**
   * Reloads the folder after it changed, keeping the reader's place: the
   * same message stays at the top of the view and the same one selected.
   */
  refresh(): void;
}

export function MessageList(props: {
  folder: FolderId;
  onSelect: (row: MessageRow | undefined) => void;
  handle?: (handle: ListHandle) => void;
}) {
  let scroller!: HTMLDivElement;

  // Plain variables, not signals: this is the scroll hot path.
  let previewLens: number[] = [];
  let layout: Layout = buildLayout([], 0);
  let width = 0;
  let pages = new Map<number, MessageRow[]>();
  let pending = new Set<number>();
  let generation = 0;
  /** Bumped when the cached rows are replaced, so late replies are dropped. */
  let epoch = 0;
  let selectedId: number | undefined;
  let refreshing = false;
  let refreshAgain = false;
  let lastTop = 0;
  // Where the view is in the content. Usually just the scroll position, but
  // browser engines cannot make an element taller than about 33 million
  // pixels, which a folder of a few hundred thousand messages exceeds. Past
  // MAX_SCROLL_HEIGHT the scrollbar covers a shorter stand-in height and
  // `virtualTop` is the real position, with rows shifted to match.
  let virtualTop = 0;
  let scaled = false;
  let lastNative = 0;
  let expectedNative = -1;
  let fineUntil = 0;
  let resync: ReturnType<typeof setTimeout> | undefined;
  let visFirst = 0;
  let visLast = -1;

  const [status, setStatus] = createSignal<"loading" | "ready" | "error">("loading");
  const [error, setError] = createSignal("");
  const [height, setHeight] = createSignal(0);
  /** slots[k] is the row shown by pooled element k, or -1 if it is idle. */
  const [slots, setSlots] = createSignal<number[]>([]);
  /** Bumped when a page of rows arrives. */
  const [dataTick, setDataTick] = createSignal(0);
  /** Bumped when row positions change. */
  const [layoutTick, setLayoutTick] = createSignal(0);
  const [selected, setSelected] = createSignal(-1);
  /** Native scroll position minus `virtualTop`. 0 unless the list is scaled. */
  const [shift, setShift] = createSignal(0);
  /** Row count, for the parts of the view that react to it. */
  const [total, setTotal] = createSignal(0);

  const count = () => layout.lines.length;
  const getRow = (i: number) => pages.get(Math.floor(i / PAGE))?.[i % PAGE];

  function fetchPage(page: number) {
    if (pages.has(page) || pending.has(page)) return;
    pending.add(page);
    const gen = generation;
    const from = epoch;
    core
      .send({ type: "listMessages", folder: props.folder, offset: page * PAGE, limit: PAGE })
      .then((r) => {
        if (gen !== generation || from !== epoch) return;
        pending.delete(page);
        pages.set(page, r.rows);
        evict(page);
        setDataTick((t) => t + 1);
      })
      .catch((e) => {
        if (gen !== generation || from !== epoch) return;
        pending.delete(page);
        setError(String(e?.message ?? e));
      });
  }

  /** Drops the cached pages farthest from where the user is. */
  function evict(keep: number) {
    if (pages.size <= MAX_CACHED_PAGES) return;
    const here = Math.floor(visFirst / PAGE);
    const far = [...pages.keys()]
      .filter((p) => p !== keep)
      .sort((a, b) => Math.abs(b - here) - Math.abs(a - here));
    for (const p of far.slice(0, pages.size - MAX_CACHED_PAGES)) pages.delete(p);
  }

  const contentHeight = () => layout.offsets[count()] ?? 0;
  const maxVirtual = () => Math.max(0, contentHeight() - scroller.clientHeight);
  const maxNative = () => Math.max(0, Math.min(contentHeight(), MAX_SCROLL_HEIGHT) - scroller.clientHeight);

  /** Call after the layout changes: sizes the scrollable area. */
  function resize() {
    scaled = contentHeight() > MAX_SCROLL_HEIGHT;
    setHeight(Math.min(contentHeight(), MAX_SCROLL_HEIGHT));
  }

  /** Moves the view to a position in the content. */
  function setTop(position: number) {
    virtualTop = Math.max(0, Math.min(maxVirtual(), position));
    const native = scaled && maxVirtual() > 0 ? (virtualTop * maxNative()) / maxVirtual() : virtualTop;
    scroller.scrollTop = native;
    lastNative = scroller.scrollTop;
    expectedNative = lastNative;
    setShift(scaled ? lastNative - virtualTop : 0);
  }

  function onScroll() {
    const native = scroller.scrollTop;
    if (!scaled) {
      virtualTop = native;
    } else if (Math.abs(native - expectedNative) < 1) {
      // Our own setTop: virtualTop is already right.
    } else {
      if (performance.now() < fineUntil) {
        // Wheel, touch and keys move the content by exactly what they
        // moved the scrollbar, so scrolling feels the same at any size.
        virtualTop += native - lastNative;
      } else {
        // Dragging the scrollbar covers the whole folder proportionally.
        virtualTop = maxNative() > 0 ? (native * maxVirtual()) / maxNative() : 0;
      }
      if (native <= 0) virtualTop = 0;
      if (native >= maxNative() - 0.5) virtualTop = maxVirtual();
      virtualTop = Math.max(0, Math.min(maxVirtual(), virtualTop));
      // Once scrolling pauses, put the scrollbar where the content is.
      clearTimeout(resync);
      resync = setTimeout(() => {
        setTop(virtualTop);
        update();
      }, 150);
    }
    expectedNative = -1;
    lastNative = native;
    setShift(scaled ? native - virtualTop : 0);
    update();
  }

  /** Marks the scrolling that follows as fine-grained (not a scrollbar drag). */
  function fine() {
    fineUntil = performance.now() + 250;
  }

  function update() {
    const n = count();
    if (n === 0) {
      visFirst = 0;
      visLast = -1;
      setSlots([]);
      return;
    }
    const top = virtualTop;
    const viewHeight = scroller.clientHeight;
    visFirst = rowAt(layout, top);
    visLast = rowAt(layout, top + viewHeight);
    const start = Math.max(0, visFirst - OVERSCAN);
    const end = Math.min(n - 1, visLast + OVERSCAN);

    // Pool size depends only on the viewport, so it is stable while scrolling.
    const pool = Math.ceil((Math.ceil(viewHeight / ROW_FIXED) + 2 * OVERSCAN + 2) / 16) * 16;
    const next = new Array<number>(pool).fill(-1);
    for (let i = start; i <= end; i++) next[i % pool] = i;
    setSlots(next);

    // Fetch what is on screen, and one page ahead in the direction of travel.
    const down = top >= lastTop;
    lastTop = top;
    const firstPage = Math.max(0, Math.floor(start / PAGE) - (down ? 0 : 1));
    const lastPage = Math.min(Math.floor((n - 1) / PAGE), Math.floor(end / PAGE) + (down ? 1 : 0));
    for (let p = firstPage; p <= lastPage; p++) fetchPage(p);
  }

  /** Recomputes row positions, keeping the top visible row where it is. */
  function relayout() {
    const anchor = visFirst;
    const into = count() > 0 ? virtualTop - layout.offsets[anchor]! : 0;
    layout = buildLayout(previewLens, width);
    setTotal(count());
    resize();
    setLayoutTick((t) => t + 1);
    if (count() > 0) {
      // The spacer must have its new height before the position can be set.
      setTop(layout.offsets[Math.min(anchor, count() - 1)]! + Math.max(0, into));
    }
    update();
  }

  async function refresh() {
    if (status() !== "ready") return;
    if (refreshing) {
      refreshAgain = true;
      return;
    }
    refreshing = true;
    const gen = generation;
    const folder = props.folder;
    try {
      // Remember what is at the top of the view, by message rather than by
      // position: positions shift when mail arrives above it.
      const top = virtualTop;
      let anchor: { id: number; into: number } | undefined;
      if (top > 4) {
        for (let i = visFirst; i <= visLast; i++) {
          const row = getRow(i);
          if (row) {
            anchor = { id: row.id, into: top - layout.offsets[i]! };
            break;
          }
        }
      }
      const locate = (id: number | undefined) =>
        id === undefined
          ? Promise.resolve(null)
          : core.send({ type: "locateMessage", folder, id }).then((r) => r.index);
      const [fresh, anchorIndex, selectedIndex] = await Promise.all([
        core.send({ type: "folderLayout", folder }),
        locate(anchor?.id),
        locate(selectedId),
      ]);
      if (gen !== generation) return;

      const nextLayout = buildLayout(fresh.previewLens, width);
      const n = nextLayout.lines.length;
      let nextTop = 0;
      if (anchor && anchorIndex !== null && anchorIndex < n) {
        nextTop = nextLayout.offsets[anchorIndex]! + anchor.into;
      } else if (anchor) {
        nextTop = Math.min(top, nextLayout.offsets[n]!);
      }

      // Fetch what will be on screen before swapping anything, so the list
      // changes in one step with no blank rows in between.
      const rows = new Map<number, MessageRow[]>();
      if (n > 0) {
        const first = Math.max(0, rowAt(nextLayout, nextTop) - OVERSCAN);
        const last = Math.min(n - 1, rowAt(nextLayout, nextTop + scroller.clientHeight) + OVERSCAN);
        const fetches: Promise<unknown>[] = [];
        for (let p = Math.floor(first / PAGE); p <= Math.floor(last / PAGE); p++) {
          fetches.push(
            core
              .send({ type: "listMessages", folder, offset: p * PAGE, limit: PAGE })
              .then((r) => rows.set(p, r.rows)),
          );
        }
        await Promise.all(fetches);
        if (gen !== generation) return;
      }

      epoch++;
      pages = rows;
      pending = new Set();
      previewLens = fresh.previewLens;
      layout = nextLayout;
      setTotal(n);
      resize();
      setLayoutTick((t) => t + 1);
      setTop(nextTop);
      lastTop = virtualTop;
      if (selectedIndex !== null || selectedId === undefined) {
        setSelected(selectedIndex ?? -1);
      } else {
        // The selected message has left the folder (deleted, moved). Select
        // whatever is now in its place, so the next Delete acts on the next
        // message, as people expect.
        const next = Math.min(selected(), n - 1);
        selectedId = undefined;
        setSelected(next);
      }
      setDataTick((t) => t + 1);
      update();
    } catch {
      // The list stays as it was; the next change tries again.
    } finally {
      refreshing = false;
      if (refreshAgain) {
        refreshAgain = false;
        void refresh();
      }
    }
  }

  function select(i: number, reveal: boolean) {
    if (i < 0 || i >= count()) return;
    setSelected(i);
    selectedId = getRow(i)?.id;
    if (!reveal) return;
    const top = layout.offsets[i]!;
    const bottom = layout.offsets[i + 1]!;
    if (top < virtualTop) setTop(top);
    else if (bottom > virtualTop + scroller.clientHeight) setTop(bottom - scroller.clientHeight);
    else return;
    update();
  }

  function onKeyDown(e: KeyboardEvent) {
    const page = Math.max(1, visLast - visFirst - 1);
    const from = selected() < 0 ? visFirst - 1 : selected();
    const to: Record<string, number> = {
      ArrowDown: from + 1,
      ArrowUp: from - 1,
      PageDown: from + page,
      PageUp: from - page,
      Home: 0,
      End: count() - 1,
    };
    const target = to[e.key];
    if (target === undefined) return;
    e.preventDefault();
    fine();
    select(Math.max(0, Math.min(count() - 1, target)), true);
  }

  // Load a folder.
  createEffect(
    on(
      () => props.folder,
      (folder) => {
        const gen = ++generation;
        selectedId = undefined;
        pages = new Map();
        pending = new Set();
        previewLens = [];
        layout = buildLayout([], width);
        visFirst = 0;
        visLast = -1;
        lastTop = 0;
        virtualTop = 0;
        scaled = false;
        setShift(0);
        setSelected(-1);
        setSlots([]);
        setTotal(0);
        setHeight(0);
        setError("");
        setStatus("loading");
        core
          .send({ type: "folderLayout", folder })
          .then((r) => {
            if (gen !== generation) return;
            previewLens = r.previewLens;
            relayout();
            setStatus("ready");
          })
          .catch((e) => {
            if (gen !== generation) return;
            setError(String(e?.message ?? e));
            setStatus("error");
          });
      },
    ),
  );

  // Tell the owner of the list which row is selected, once its data is here.
  createEffect(() => {
    dataTick();
    const i = selected();
    const row = i >= 0 ? getRow(i) : undefined;
    if (row) selectedId = row.id;
    props.onSelect(row);
  });

  onMount(() => {
    width = scroller.clientWidth;
    const observer = new ResizeObserver(() => {
      const w = scroller.clientWidth;
      if (w !== width) {
        width = w;
        relayout();
      } else {
        update();
      }
    });
    observer.observe(scroller);
    onCleanup(() => {
      observer.disconnect();
      clearTimeout(resync);
    });
    props.handle?.({
      scroller,
      rowCount: count,
      visibleRange: () => [visFirst, visLast],
      selectedRow: () => (selected() >= 0 ? getRow(selected()) : undefined),
      select: (index) => select(index, true),
      refresh: () => void refresh(),
      blankVisible() {
        let blank = 0;
        for (let i = visFirst; i <= visLast; i++) if (!getRow(i)) blank++;
        return blank;
      },
    });
  });

  return (
    <div
      class="list"
      ref={scroller}
      tabindex="0"
      role="listbox"
      aria-label="Messages"
      onScroll={onScroll}
      onWheel={fine}
      onTouchMove={fine}
      onKeyDown={onKeyDown}
    >
      <div class="list-spacer" style={{ height: `${height()}px` }}>
        <Index each={slots()}>
          {(index) => {
            const row = createMemo(() => {
              dataTick();
              return index() >= 0 ? getRow(index()) : undefined;
            });
            const top = () => (layoutTick(), index() >= 0 ? layout.offsets[index()]! + shift() : 0);
            const lines = () => (layoutTick(), index() >= 0 ? layout.lines[index()]! : 0);
            return (
              <div
                class="row"
                role="option"
                aria-selected={index() >= 0 && index() === selected()}
                classList={{
                  idle: index() < 0,
                  selected: index() >= 0 && index() === selected(),
                  unread: row()?.unread === true,
                }}
                style={{
                  transform: `translateY(${top()}px)`,
                  height: `${ROW_FIXED + lines() * LINE}px`,
                }}
                onMouseDown={() => select(index(), false)}
              >
                <Show when={row()} fallback={<div class="row-loading" />}>
                  {(r) => (
                    <>
                      <div class="row-top">
                        <span class="row-dot" />
                        <span class="row-sender">{r().senderName}</span>
                        <Show when={r().flagged}>
                          <span class="row-flag" title="Flagged">
                            ⚑
                          </span>
                        </Show>
                        <Show when={r().hasAttachment}>
                          <span class="row-clip" title="Has attachment">
                            📎
                          </span>
                        </Show>
                        <span class="row-date">{formatDate(r().date)}</span>
                      </div>
                      <div class="row-subject">{r().subject}</div>
                      <div
                        class="row-preview"
                        style={{ "-webkit-line-clamp": lines(), height: `${lines() * LINE}px` }}
                      >
                        {r().preview}
                      </div>
                    </>
                  )}
                </Show>
              </div>
            );
          }}
        </Index>
      </div>
      <Show when={status() === "ready" && total() === 0}>
        <div class="list-note">This folder is empty.</div>
      </Show>
      <Show when={status() === "loading"}>
        <div class="list-note">Loading…</div>
      </Show>
      <Show when={error()}>
        <div class="list-note list-error">Could not load messages: {error()}</div>
      </Show>
    </div>
  );
}
