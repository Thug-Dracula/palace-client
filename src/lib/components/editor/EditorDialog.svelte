<script lang="ts">
  /**
   * The prop editor shell.
   *
   * Owns the whole editor surface: the File/Edit/Image/View/Prop ID menu bar,
   * the canvas and its zoom control, the panel dock, and the status line. All
   * pixel work stays in Rust: this component asks `api` for typed state and one
   * already-composited PNG per frame, and sends commands for open/undo/redo and
   * frame operations. Zoom and pan live in `$lib/editorView` and only move the
   * <img> on screen.
   *
   * Keyboard: a focus trap keeps Tab inside the editor (and inside whichever
   * overlay is open), Escape closes the topmost overlay, and the shortcuts
   * resolved by `$lib/components/editor/a11y` cover undo/redo, zoom, save and
   * tool-panel switching. All copy comes from `./strings`.
   */
  import { onDestroy, onMount, tick } from "svelte";

  import * as api from "../../api";
  import type {
    BagPropEntry,
    EditorGuideGeometry,
    EditorGuideState,
    EditorOutcome,
    EditorState,
  } from "../../api";
  import {
    PANEL_TABS,
    actualSizeTransform,
    canvasSize,
    clampPan,
    cyclePanel,
    fitTransform,
    formatZoom,
    frameInfo,
    hexToRgba,
    onionGhostOpacity,
    originLabel,
    panelLabel,
    propIdOf,
    sameLayerPoint,
    saveLabel,
    zoomStepAt,
    type CanvasGuides,
    type GuideGhost,
    type LayerPoint,
    type PaintState,
    type PanelId,
    type ViewTransform,
  } from "../../editorView";
  import EditorCanvas from "./EditorCanvas.svelte";
  import AdjustPanel from "./panels/AdjustPanel.svelte";
  import FramesPanel from "./panels/FramesPanel.svelte";
  import GuidesPanel from "./panels/GuidesPanel.svelte";
  import PaintPanel from "./panels/PaintPanel.svelte";
  import SizePanel from "./panels/SizePanel.svelte";
  import TextPanel from "./panels/TextPanel.svelte";
  import TransformPanel from "./panels/TransformPanel.svelte";
  import type { PanelNoticeKind } from "./panels/contract";
  import { editorCommand, focusFirst, trapTabKey } from "./a11y";
  import { defaultSaveForm, isAnimatedSaveBlocked, toSaveOptions } from "./save";
  import { BAG, EDITOR, MESSAGES, withShortcut } from "./strings";
  import "./ui.css";

  let { onclose }: { onclose: () => void } = $props();

  const MENU_DEFS = [
    { id: "file", label: EDITOR.menus.file },
    { id: "edit", label: EDITOR.menus.edit },
    { id: "image", label: EDITOR.menus.image },
    { id: "view", label: EDITOR.menus.view },
    { id: "prop", label: EDITOR.menus.prop },
  ] as const;
  type MenuId = (typeof MENU_DEFS)[number]["id"];

  let dialog: HTMLDivElement | undefined = $state();
  let pickerEl: HTMLDivElement | undefined = $state();
  let confirmEl: HTMLDivElement | undefined = $state();
  let fileInput: HTMLInputElement | undefined = $state();

  // The open document. Named `doc`, not `state`: a local binding called `state`
  // makes Svelte read `$state` as a store subscription to it, breaking the rune.
  let doc = $state<EditorState | null>(null);
  let src = $state<string | null>(null);
  let view = $state<ViewTransform>({ scale: 1, x: 0, y: 0 });
  let viewport = $state({ w: 0, h: 0 });
  let pendingFit = $state(false);

  let activePanel = $state<PanelId>("paint");
  let paint = $state<PaintState>({
    tool: "brush",
    color: "#ff0000",
    size: 1,
    layer: 0,
    allFrames: false,
  });
  // Guide state lives in the Rust session; this is the shell's read-only copy
  // for the canvas overlay, refreshed whenever the document or switches change.
  let guideState = $state<EditorGuideState | null>(null);
  let guideGeometry = $state<EditorGuideGeometry | null>(null);
  let onionGhosts = $state<GuideGhost[]>([]);
  let ghostUrls = new Map<number, string>();
  const canvasGuides = $derived<CanvasGuides | null>(
    guideState && guideGeometry
      ? { geometry: guideGeometry, toggles: guideState.toggles, onion: onionGhosts }
      : null,
  );
  const tool = $derived(paint.tool === "brush" ? "Brush" : "Eraser");
  // Pointer painting is armed only on the Paint tab with a document open; every
  // other tab keeps the canvas's original left-drag-to-pan behaviour.
  const painting = $derived(activePanel === "paint" && doc !== null);
  let busy = $state(false);
  let saving = $state(false);
  let error = $state<string | null>(null);
  let status = $state<{ text: string; kind: PanelNoticeKind } | null>(null);

  let openMenu = $state<MenuId | null>(null);
  let newOpen = $state(false);
  let newWidth = $state(64);
  let newHeight = $state(64);
  let confirmClose = $state(false);

  let bagOpen = $state(false);
  let bagEntries = $state<BagPropEntry[] | null>(null);
  let bagLoading = $state(false);
  let bagError = $state<string | null>(null);
  let bagQuery = $state("");

  // Save form lives here (not in the Frames panel) so Ctrl+S works from any
  // panel, and so switching tabs never loses what the user typed.
  let saveForm = $state(defaultSaveForm());

  let frameToken = 0;
  let objectUrl: string | null = null;
  let noticeTimer: ReturnType<typeof setTimeout> | null = null;
  // Where focus returns after a transient overlay closes.
  let restoreFocus: HTMLElement | null = null;

  const originText = $derived(originLabel(doc?.origin));
  const propId = $derived(propIdOf(doc?.origin));
  const sizeLabel = $derived(canvasSize(doc));
  const frameLabel = $derived(frameInfo(doc));
  const zoomLabel = $derived(formatZoom(view.scale));
  const imageWidth = $derived(doc?.width ?? 0);
  const imageHeight = $derived(doc?.height ?? 0);
  const animatedDocument = $derived(isAnimatedSaveBlocked(doc?.frame_count ?? 1));
  const saveDisabled = $derived(busy || saving || !doc || animatedDocument);

  const bagVisible = $derived.by(() => {
    const all = bagEntries ?? [];
    const query = bagQuery.trim().toLowerCase();
    const filtered =
      query === ""
        ? all
        : all.filter(
            (entry) =>
              (entry.name ?? "").toLowerCase().includes(query) ||
              String(entry.id).includes(query) ||
              entry.collection.toLowerCase().includes(query),
          );
    return filtered.slice(0, 240);
  });

  function message(cause: unknown): string {
    return cause instanceof Error ? cause.message : String(cause);
  }

  function pushNotice(text: string, kind: PanelNoticeKind = "info"): void {
    status = { text, kind };
    // A failure must not scroll away with the status line: pin it in the
    // persistent alert bar until the user dismisses it.
    if (kind === "error") {
      error = text;
    }
    if (noticeTimer) {
      clearTimeout(noticeTimer);
    }
    noticeTimer = setTimeout(() => {
      status = null;
      noticeTimer = null;
    }, 5000);
  }

  /** Replace the canvas image with the current frame's PNG, ignoring stale loads. */
  async function loadFrame(target: EditorState): Promise<void> {
    const token = ++frameToken;
    try {
      const bytes = await api.editorFramePng(target.frame_index);
      if (token !== frameToken) {
        return;
      }
      const blob = new Blob([new Uint8Array(bytes)], { type: "image/png" });
      const url = URL.createObjectURL(blob);
      if (objectUrl) {
        URL.revokeObjectURL(objectUrl);
      }
      objectUrl = url;
      src = url;
    } catch (cause) {
      if (token === frameToken) {
        error = message(cause);
      }
    }
  }

  function clearFrame(): void {
    frameToken += 1;
    if (objectUrl) {
      URL.revokeObjectURL(objectUrl);
      objectUrl = null;
    }
    src = null;
    guideState = null;
    guideGeometry = null;
    onionGhosts = [];
    revokeGhostUrls();
  }

  function revokeGhostUrls(): void {
    for (const url of ghostUrls.values()) {
      URL.revokeObjectURL(url);
    }
    ghostUrls = new Map();
  }

  async function ghostUrl(index: number): Promise<string | null> {
    const cached = ghostUrls.get(index);
    if (cached) {
      return cached;
    }
    try {
      const bytes = await api.editorFramePng(index);
      const blob = new Blob([new Uint8Array(bytes)], { type: "image/png" });
      const url = URL.createObjectURL(blob);
      ghostUrls.set(index, url);
      return url;
    } catch {
      return null;
    }
  }

  /** Re-read the guide switches, geometry and onion neighbours from Rust. */
  async function reloadGuides(): Promise<void> {
    if (!doc) {
      guideState = null;
      guideGeometry = null;
      onionGhosts = [];
      revokeGhostUrls();
      return;
    }
    try {
      const state = await api.editorGuideState();
      guideState = state;
      guideGeometry = await api.editorGuideGeometry();
      if (!state.toggles.onion) {
        onionGhosts = [];
        revokeGhostUrls();
        return;
      }
      const neighbours = await api.editorOnionNeighbours();
      const wanted = new Set(neighbours.map((neighbour) => neighbour.index));
      for (const [index, url] of ghostUrls) {
        if (!wanted.has(index)) {
          URL.revokeObjectURL(url);
          ghostUrls.delete(index);
        }
      }
      const ghosts: GuideGhost[] = [];
      for (const neighbour of neighbours) {
        const src = await ghostUrl(neighbour.index);
        if (src) {
          ghosts.push({
            index: neighbour.index,
            src,
            opacity: onionGhostOpacity(neighbour.offset),
          });
        }
      }
      onionGhosts = ghosts;
    } catch {
      guideState = null;
      guideGeometry = null;
      onionGhosts = [];
      revokeGhostUrls();
    }
  }

  async function guard(action: () => Promise<void>): Promise<void> {
    if (busy) {
      return;
    }
    busy = true;
    error = null;
    try {
      await action();
    } catch (cause) {
      error = message(cause);
    } finally {
      busy = false;
    }
  }

  /** Apply a command outcome: adopt its state and refresh the canvas if it changed. */
  async function runOutcome(operation: () => Promise<EditorOutcome>): Promise<void> {
    await guard(async () => {
      const outcome = await operation();
      doc = outcome.state;
      if (outcome.changed) {
        await loadFrame(outcome.state);
      }
      await reloadGuides();
    });
  }

  async function refresh(): Promise<void> {
    await guard(async () => {
      const next = await api.editorState();
      doc = next;
      if (next) {
        await loadFrame(next);
      } else {
        clearFrame();
      }
      await reloadGuides();
    });
  }

  async function openBlank(): Promise<void> {
    await guard(async () => {
      const width = Math.max(1, Math.min(4096, Math.round(newWidth)));
      const height = Math.max(1, Math.min(4096, Math.round(newHeight)));
      const next = await api.editorOpenBlank(width, height);
      newWidth = width;
      newHeight = height;
      doc = next;
      newOpen = false;
      pendingFit = true;
      await loadFrame(next);
      await reloadGuides();
    });
  }

  function pickImage(): void {
    error = null;
    fileInput?.click();
  }

  async function onImageChosen(event: Event): Promise<void> {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    input.value = "";
    if (!file) {
      return;
    }
    await guard(async () => {
      const buffer = await file.arrayBuffer();
      const bytes = Array.from(new Uint8Array(buffer));
      const next = await api.editorOpenImage(bytes);
      doc = next;
      pendingFit = true;
      await loadFrame(next);
      await reloadGuides();
    });
  }

  async function loadBagPicker(): Promise<void> {
    bagLoading = true;
    bagError = null;
    try {
      bagEntries = await api.bagCatalog();
    } catch (cause) {
      bagError = MESSAGES.bagLoadFailedDetail(message(cause));
    } finally {
      bagLoading = false;
    }
  }

  async function openBagPicker(): Promise<void> {
    openMenu = null;
    restoreFocus = document.activeElement as HTMLElement | null;
    bagOpen = true;
    await tick();
    focusFirst(pickerEl);
    if (bagEntries === null) {
      await loadBagPicker();
    }
  }

  function closeBagPicker(): void {
    bagOpen = false;
    bagError = null;
    restoreFocus?.focus();
    restoreFocus = null;
  }

  async function openBagEntry(entry: BagPropEntry): Promise<void> {
    await guard(async () => {
      const next = await api.editorOpenBagProp(entry.collection, entry.id, entry.crc);
      doc = next;
      closeBagPicker();
      pendingFit = true;
      await loadFrame(next);
      await reloadGuides();
    });
  }

  const undo = (): Promise<void> => runOutcome(() => api.editorUndo());
  const redo = (): Promise<void> => runOutcome(() => api.editorRedo());
  const addFrame = (): Promise<void> => runOutcome(() => api.editorAddFrame());
  const duplicateFrame = (): Promise<void> =>
    runOutcome(() => api.editorDuplicateFrame(doc?.frame_index ?? 0));
  const deleteFrame = (): Promise<void> =>
    runOutcome(() => api.editorDeleteFrame(doc?.frame_index ?? 0));
  const selectFrame = (index: number): Promise<void> =>
    runOutcome(() => api.editorSelectFrame(index));

  /** Save the current document into My Bag using the shared save form. */
  async function saveToBag(): Promise<void> {
    if (saveDisabled || !doc) {
      if (animatedDocument) {
        pushNotice(MESSAGES.animatedSaveBlocked(doc?.frame_count ?? 0), "warn");
      }
      return;
    }
    saving = true;
    error = null;
    try {
      const result = await api.editorSaveProp(toSaveOptions(saveForm));
      pushNotice(result.message);
      await refresh();
    } catch (cause) {
      pushNotice(message(cause), "error");
    } finally {
      saving = false;
    }
  }

  function applyView(next: ViewTransform): void {
    if (doc && doc.width > 0 && viewport.w > 0 && viewport.h > 0) {
      view = clampPan(next, doc.width, doc.height, viewport.w, viewport.h);
    } else {
      view = next;
    }
  }

  function zoomIn(): void {
    if (!doc || viewport.w === 0) {
      return;
    }
    view = clampPan(
      zoomStepAt(view, 1, viewport.w / 2, viewport.h / 2),
      doc.width,
      doc.height,
      viewport.w,
      viewport.h,
    );
  }

  function zoomOut(): void {
    if (!doc || viewport.w === 0) {
      return;
    }
    view = clampPan(
      zoomStepAt(view, -1, viewport.w / 2, viewport.h / 2),
      doc.width,
      doc.height,
      viewport.w,
      viewport.h,
    );
  }

  function fit(): void {
    if (!doc || viewport.w === 0) {
      return;
    }
    applyView(fitTransform(doc.width, doc.height, viewport.w, viewport.h));
  }

  function actualSize(): void {
    if (!doc || viewport.w === 0) {
      return;
    }
    applyView(actualSizeTransform(doc.width, doc.height, viewport.w, viewport.h));
  }

  function onCanvasChange(next: ViewTransform): void {
    view = next;
  }

  function onCanvasMetrics(width: number, height: number): void {
    viewport = { w: width, h: height };
  }

  // -- Pointer painting ---------------------------------------------------
  //
  // A drag arrives as one `begin` then many `extend`s. Commands are chained so
  // they reach Rust in order, and extensions coalesce to the latest pointer
  // position: when the UI is slower than the pointer, a fast drag becomes a few
  // longer segments (Rust connects them with its Bresenham walk) instead of a
  // queue of hundreds. The Rust session pushes one undo snapshot per drag.
  let paintChain: Promise<void> = Promise.resolve();
  let strokeSeq = 0;
  let strokeSent: LayerPoint | null = null;
  let strokeLatest: LayerPoint | null = null;
  let pumpQueued = false;

  function strokeOptions(from: LayerPoint, to: LayerPoint): api.EditorStrokeSessionOptions {
    return {
      mode: paint.tool === "brush" ? "brush" : "erase",
      fromX: from.x,
      fromY: from.y,
      toX: to.x,
      toY: to.y,
      color: hexToRgba(paint.color),
      size: paint.size,
      layer: paint.layer,
      allFrames: paint.allFrames,
    };
  }

  function enqueuePaint(task: () => Promise<void>): void {
    paintChain = paintChain.then(task).catch((cause) => {
      error = message(cause);
    });
  }

  async function adoptPaintOutcome(outcome: EditorOutcome): Promise<void> {
    doc = outcome.state;
    if (outcome.changed) {
      await loadFrame(outcome.state);
    }
  }

  function pumpStroke(): void {
    if (pumpQueued) {
      return;
    }
    pumpQueued = true;
    const generation = strokeSeq;
    enqueuePaint(async () => {
      pumpQueued = false;
      if (generation !== strokeSeq) {
        return;
      }
      const from = strokeSent;
      const to = strokeLatest;
      if (!from || !to || sameLayerPoint(from, to)) {
        return;
      }
      const outcome = await api.editorExtendStroke(strokeOptions(from, to));
      if (generation !== strokeSeq) {
        return;
      }
      strokeSent = to;
      await adoptPaintOutcome(outcome);
      if (generation === strokeSeq && strokeLatest && !sameLayerPoint(strokeSent, strokeLatest)) {
        pumpStroke();
      }
    });
  }

  function onPaintBegin(point: LayerPoint): void {
    strokeSeq += 1;
    const generation = strokeSeq;
    strokeSent = point;
    strokeLatest = point;
    pumpQueued = false;
    enqueuePaint(async () => {
      if (generation !== strokeSeq) {
        return;
      }
      const outcome = await api.editorBeginStroke(strokeOptions(point, point));
      if (generation !== strokeSeq) {
        return;
      }
      await adoptPaintOutcome(outcome);
    });
  }

  function onPaintExtend(_from: LayerPoint, to: LayerPoint): void {
    strokeLatest = to;
    pumpStroke();
  }

  function onPaintEnd(): void {
    // Leave the pending target in place: the last queued extension still needs
    // to flush. The next `begin` resets the drag.
  }

  async function requestClose(): Promise<void> {
    openMenu = null;
    if (doc?.dirty) {
      restoreFocus = document.activeElement as HTMLElement | null;
      confirmClose = true;
      await tick();
      focusFirst(confirmEl);
      return;
    }
    onclose();
  }

  function cancelClose(): void {
    confirmClose = false;
    restoreFocus?.focus();
    restoreFocus = null;
  }

  function discardAndClose(): void {
    confirmClose = false;
    onclose();
  }

  async function copyPropId(): Promise<void> {
    if (propId === null) {
      return;
    }
    try {
      await navigator.clipboard.writeText(String(propId));
      pushNotice(MESSAGES.copiedPropId(propId));
    } catch {
      pushNotice(MESSAGES.clipboardUnavailable(propId), "warn");
    }
  }

  function toggleMenu(id: MenuId): void {
    openMenu = openMenu === id ? null : id;
  }

  function hoverMenu(id: MenuId): void {
    if (openMenu !== null && openMenu !== id) {
      openMenu = id;
    }
  }

  function pick(action: () => void): void {
    openMenu = null;
    action();
  }

  function closePick(): void {
    openMenu = null;
  }

  /** Select a tool panel and move focus to its tab so the change is announced. */
  async function selectPanel(id: PanelId): Promise<void> {
    activePanel = id;
    await tick();
    dialog?.querySelector<HTMLButtonElement>(`[data-panel="${id}"]`)?.focus();
  }

  /** Run a shortcut resolved by the pure `editorCommand` map. */
  function runCommand(command: NonNullable<ReturnType<typeof editorCommand>>): void {
    switch (command.kind) {
      case "undo":
        void undo();
        break;
      case "redo":
        void redo();
        break;
      case "zoom-in":
        zoomIn();
        break;
      case "zoom-out":
        zoomOut();
        break;
      case "fit":
        fit();
        break;
      case "actual-size":
        actualSize();
        break;
      case "save":
        void saveToBag();
        break;
      case "panel-cycle":
        void selectPanel(cyclePanel(activePanel, command.delta));
        break;
      case "panel-select":
        void selectPanel(command.id);
        break;
    }
  }

  /** The element the focus trap should hold: the topmost overlay, else the dialog. */
  function trapRoot(): HTMLElement | undefined {
    if (confirmClose) {
      return confirmEl;
    }
    if (bagOpen) {
      return pickerEl;
    }
    return dialog;
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      if (bagOpen) {
        closeBagPicker();
      } else if (confirmClose) {
        cancelClose();
      } else if (newOpen) {
        newOpen = false;
      } else if (openMenu) {
        openMenu = null;
      } else {
        void requestClose();
      }
      return;
    }

    if (event.key === "Tab") {
      if (trapTabKey(event, trapRoot())) {
        event.stopPropagation();
      }
      return;
    }

    const target = event.target as HTMLElement | null;
    const tag = target?.tagName;
    const inField =
      tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA" || target?.isContentEditable === true;

    const command = editorCommand(event);
    if (!command) {
      // Swallow unhandled keys so the room's shortcuts never fire behind the modal.
      event.stopPropagation();
      return;
    }
    if (inField && command.kind !== "save" && command.kind !== "panel-cycle" && command.kind !== "panel-select") {
      // Let a focused text field keep its native Ctrl+Z / zoom behaviour.
      event.stopPropagation();
      return;
    }
    event.preventDefault();
    runCommand(command);
    event.stopPropagation();
  }

  onMount(() => {
    dialog?.focus();
    void (async () => {
      try {
        const existing = await api.editorState();
        if (existing) {
          doc = existing;
          pendingFit = true;
          await loadFrame(existing);
          await reloadGuides();
        }
      } catch (cause) {
        error = message(cause);
      }
    })();
  });

  onDestroy(() => {
    clearFrame();
    if (noticeTimer) {
      clearTimeout(noticeTimer);
      noticeTimer = null;
    }
  });

  // Fit a freshly opened document as soon as both the image and the canvas box
  // have real sizes (the box is measured by ResizeObserver, after layout).
  $effect(() => {
    if (!pendingFit || !doc || viewport.w === 0 || viewport.h === 0) {
      return;
    }
    view = fitTransform(doc.width, doc.height, viewport.w, viewport.h);
    pendingFit = false;
  });
</script>

<div class="dialog-backdrop">
  <div
    class="dialog editor-dialog"
    role="dialog"
    aria-modal="true"
    aria-label={EDITOR.dialogLabel}
    tabindex="-1"
    bind:this={dialog}
    onkeydown={onKeydown}
  >
    <div class="editor-menubar">
      <span class="editor-title">{EDITOR.title}</span>
      <nav class="menu-bar" aria-label={EDITOR.menuLabel}>
        {#each MENU_DEFS as menu (menu.id)}
          <div class="menu-root">
            <button
              class="menu-top"
              class:open={openMenu === menu.id}
              type="button"
              aria-haspopup="menu"
              aria-expanded={openMenu === menu.id}
              onclick={() => toggleMenu(menu.id)}
              onmouseenter={() => hoverMenu(menu.id)}
            >
              {menu.label}
            </button>

            {#if openMenu === menu.id}
              <div class="menu-pop" role="menu">
                {#if menu.id === "file"}
                  <button class="menu-item" type="button" role="menuitem" onclick={() => pick(() => (newOpen = !newOpen))}>
                    {EDITOR.file.newCanvas}
                  </button>
                  <button class="menu-item" type="button" role="menuitem" disabled={busy} onclick={() => pick(pickImage)}>
                    {EDITOR.file.openImage}
                  </button>
                  <button class="menu-item" type="button" role="menuitem" disabled={busy} onclick={() => pick(() => void openBagPicker())}>
                    {EDITOR.file.openBagProp}
                  </button>
                  <div class="menu-sep"></div>
                  <button class="menu-item" type="button" role="menuitem" onclick={() => pick(() => void requestClose())}>
                    {EDITOR.file.close}
                  </button>
                {:else if menu.id === "edit"}
                  <button
                    class="menu-item"
                    type="button"
                    role="menuitem"
                    title={withShortcut(EDITOR.edit.undo, EDITOR.shortcuts.undo)}
                    disabled={busy || !doc?.can_undo}
                    onclick={() => pick(() => void undo())}
                  >
                    {EDITOR.edit.undo} <span class="menu-key">{EDITOR.shortcuts.undo}</span>
                  </button>
                  <button
                    class="menu-item"
                    type="button"
                    role="menuitem"
                    title={withShortcut(EDITOR.edit.redo, EDITOR.shortcuts.redo)}
                    disabled={busy || !doc?.can_redo}
                    onclick={() => pick(() => void redo())}
                  >
                    {EDITOR.edit.redo} <span class="menu-key">{EDITOR.shortcuts.redo}</span>
                  </button>
                {:else if menu.id === "image"}
                  <button
                    class="menu-item"
                    type="button"
                    role="menuitem"
                    disabled={busy || !doc}
                    onclick={() => pick(() => void addFrame())}
                  >
                    {EDITOR.image.addFrame}
                  </button>
                  <button
                    class="menu-item"
                    type="button"
                    role="menuitem"
                    disabled={busy || !doc}
                    onclick={() => pick(() => void duplicateFrame())}
                  >
                    {EDITOR.image.duplicateFrame}
                  </button>
                  <button
                    class="menu-item"
                    type="button"
                    role="menuitem"
                    disabled={busy || !doc || doc.frame_count <= 1}
                    onclick={() => pick(() => void deleteFrame())}
                  >
                    {EDITOR.image.deleteFrame}
                  </button>
                {:else if menu.id === "view"}
                  <button class="menu-item" type="button" role="menuitem" title={withShortcut(EDITOR.view.zoomIn, EDITOR.shortcuts.zoomIn)} disabled={!doc} onclick={() => pick(zoomIn)}>
                    {EDITOR.view.zoomIn} <span class="menu-key">{EDITOR.shortcuts.zoomIn}</span>
                  </button>
                  <button class="menu-item" type="button" role="menuitem" title={withShortcut(EDITOR.view.zoomOut, EDITOR.shortcuts.zoomOut)} disabled={!doc} onclick={() => pick(zoomOut)}>
                    {EDITOR.view.zoomOut} <span class="menu-key">{EDITOR.shortcuts.zoomOut}</span>
                  </button>
                  <button class="menu-item" type="button" role="menuitem" title={withShortcut(EDITOR.view.actualSize, EDITOR.shortcuts.actualSize)} disabled={!doc} onclick={() => pick(actualSize)}>
                    {EDITOR.view.actualSize} <span class="menu-key">{EDITOR.shortcuts.actualSize}</span>
                  </button>
                  <button class="menu-item" type="button" role="menuitem" title={withShortcut(EDITOR.view.fitWindow, EDITOR.shortcuts.fit)} disabled={!doc} onclick={() => pick(fit)}>
                    {EDITOR.view.fitWindow} <span class="menu-key">{EDITOR.shortcuts.fit}</span>
                  </button>
                {:else if menu.id === "prop"}
                  <div class="menu-info">
                    <div><span>{EDITOR.prop.origin}</span><b>{originText}</b></div>
                    <div><span>{EDITOR.prop.propId}</span><b>{propId ?? "—"}</b></div>
                  </div>
                  <button
                    class="menu-item"
                    type="button"
                    role="menuitem"
                    disabled={propId === null}
                    onclick={() => pick(() => void copyPropId())}
                  >
                    {EDITOR.prop.copyId}
                  </button>
                  <button class="menu-item" type="button" role="menuitem" disabled title={EDITOR.prop.editIdHint}>
                    {EDITOR.prop.editId}
                  </button>
                {/if}
              </div>
            {/if}
          </div>
        {/each}
      </nav>

      <span class="spacer"></span>
      {#if busy || saving}<span class="editor-working">{EDITOR.working}</span>{/if}
      <span class="save-ind" class:dirty={doc?.dirty} class:clean={doc && !doc.dirty}>
        <span class="dot"></span>{saveLabel(doc)}
      </span>
      <button class="dialog-x" type="button" aria-label={EDITOR.closeLabel} onclick={() => void requestClose()}>×</button>
    </div>

    {#if openMenu}
      <div class="menu-catch" role="presentation" onclick={closePick}></div>
    {/if}

    {#if newOpen}
      <div class="new-bar">
        <span class="new-label">{EDITOR.newBar.label}</span>
        <label>
          {EDITOR.newBar.width}
          <input type="number" min="1" max="4096" step="1" bind:value={newWidth} />
        </label>
        <label>
          {EDITOR.newBar.height}
          <input type="number" min="1" max="4096" step="1" bind:value={newHeight} />
        </label>
        <button class="btn primary" type="button" disabled={busy} onclick={() => void openBlank()}>
          {EDITOR.newBar.create}
        </button>
        <button class="btn ghost" type="button" onclick={() => (newOpen = false)}>{EDITOR.newBar.cancel}</button>
        <span class="new-hint">{EDITOR.newBar.maxHint}</span>
      </div>
    {/if}

    {#if error}
      <div class="ui-alert error editor-error" role="alert">
        <span class="ui-alert-text">{error}</span>
        <button class="dialog-x" type="button" aria-label={EDITOR.dismissError} onclick={() => (error = null)}>×</button>
      </div>
    {/if}

    <div class="editor-body">
      <div class="editor-main">
        <div class="canvas-toolbar">
          <div class="seg" role="group" aria-label={EDITOR.zoom.group}>
            <button class="btn" type="button" aria-label={EDITOR.zoom.out} title={withShortcut(EDITOR.zoom.out, EDITOR.shortcuts.zoomOut)} disabled={!doc} onclick={zoomOut}>−</button>
            <button class="btn" type="button" title={withShortcut(EDITOR.zoom.fit, EDITOR.shortcuts.fit)} disabled={!doc} onclick={fit}>{EDITOR.zoom.fit}</button>
            <button class="btn" type="button" title={withShortcut(EDITOR.view.actualSize, EDITOR.shortcuts.actualSize)} disabled={!doc} onclick={actualSize}>{EDITOR.zoom.actualSize}</button>
            <button class="btn" type="button" aria-label={EDITOR.zoom.in} title={withShortcut(EDITOR.zoom.in, EDITOR.shortcuts.zoomIn)} disabled={!doc} onclick={zoomIn}>+</button>
          </div>
          <span class="readout" aria-label={EDITOR.zoom.readout}>{zoomLabel}</span>

          <div class="seg frame-nav" role="group" aria-label={EDITOR.frames.group}>
            <button
              class="btn"
              type="button"
              aria-label={EDITOR.frames.previous}
              disabled={!doc || doc.frame_index <= 0}
              onclick={() => void selectFrame((doc?.frame_index ?? 0) - 1)}
            >
              ◀
            </button>
            <button
              class="btn"
              type="button"
              aria-label={EDITOR.frames.next}
              disabled={!doc || doc.frame_index >= doc.frame_count - 1}
              onclick={() => void selectFrame((doc?.frame_index ?? 0) + 1)}
            >
              ▶
            </button>
          </div>
          <span class="frame-readout">{frameLabel}</span>

          <span class="spacer"></span>
          <span class="canvas-hint">{EDITOR.canvasHint}</span>
        </div>

        <EditorCanvas
          {src}
          {imageWidth}
          {imageHeight}
          {view}
          {painting}
          guides={canvasGuides}
          onchange={onCanvasChange}
          onmetrics={onCanvasMetrics}
          onpaintbegin={onPaintBegin}
          onpaintextend={onPaintExtend}
          onpaintend={onPaintEnd}
        />
      </div>

      <aside class="editor-panels" aria-label={EDITOR.panels.region}>
        <div class="panel-tabs" role="tablist" aria-label={EDITOR.panels.tabs}>
          {#each PANEL_TABS as tab, index (tab.id)}
            <button
              class="panel-tab"
              class:on={activePanel === tab.id}
              type="button"
              role="tab"
              data-panel={tab.id}
              title={`${tab.label} (Alt+${index + 1})`}
              aria-selected={activePanel === tab.id}
              onclick={() => void selectPanel(tab.id)}
            >
              {tab.label}
            </button>
          {/each}
        </div>
        <div class="panel-body" role="tabpanel" aria-label={panelLabel(activePanel)}>
          {#if activePanel === "paint"}
            <PaintPanel state={doc} {busy} active={true} notify={pushNotice} refresh={refresh} bind:paint />
          {:else if activePanel === "transform"}
            <TransformPanel state={doc} {busy} active={true} notify={pushNotice} refresh={refresh} />
          {:else if activePanel === "adjust"}
            <AdjustPanel state={doc} {busy} active={true} notify={pushNotice} refresh={refresh} />
          {:else if activePanel === "size"}
            <SizePanel state={doc} {busy} active={true} notify={pushNotice} refresh={refresh} />
          {:else if activePanel === "frames"}
            <FramesPanel
              state={doc}
              busy={busy || saving}
              active={true}
              notify={pushNotice}
              refresh={refresh}
              form={saveForm}
              onsave={() => void saveToBag()}
            />
          {:else if activePanel === "text"}
            <TextPanel state={doc} {busy} active={true} notify={pushNotice} refresh={refresh} />
          {:else}
            <GuidesPanel
              state={doc}
              {busy}
              active={true}
              notify={pushNotice}
              refresh={refresh}
              guideRefresh={reloadGuides}
            />
          {/if}
        </div>
      </aside>
    </div>

    <div class="editor-status" role="status" aria-live="polite">
      <span class="status-item" title={EDITOR.status.tool}>{tool}</span>
      <span class="status-sep"></span>
      <span class="status-item" title={EDITOR.status.prop}>{originText}</span>
      {#if propId !== null}
        <span class="status-sep"></span>
        <span class="status-item">{EDITOR.prop.propId} {propId}</span>
      {/if}
      <span class="status-sep"></span>
      <span class="status-item">{sizeLabel}</span>
      <span class="status-sep"></span>
      <span class="status-item">{frameLabel}</span>
      {#if status}
        <span class="status-sep"></span>
        <span class="status-note" class:warn={status.kind === "warn"} class:error={status.kind === "error"}>
          {status.text}
        </span>
      {/if}
      <span class="spacer"></span>
      <span class="status-item depth">undo {doc?.undo_depth ?? 0} · redo {doc?.redo_depth ?? 0}</span>
    </div>

    {#if bagOpen}
      <div class="editor-overlay">
        <div
          class="picker"
          role="dialog"
          aria-modal="true"
          aria-label={EDITOR.file.openBagProp}
          tabindex="-1"
          bind:this={pickerEl}
        >
          <div class="dialog-head">
            <span>{EDITOR.file.openBagProp}</span>
            <button class="dialog-x" type="button" aria-label={EDITOR.closeBagPicker} onclick={closeBagPicker}>×</button>
          </div>
          <div class="picker-body">
            <input
              class="picker-search"
              type="search"
              placeholder={BAG.filters.searchPlaceholder}
              aria-label={BAG.filters.searchLabel}
              bind:value={bagQuery}
            />
            {#if bagLoading}
              <p class="picker-hint ui-loading" role="status">{BAG.loading}</p>
            {:else if bagError}
              <div class="ui-alert error">
                <span class="ui-alert-text">{bagError}</span>
                <button class="btn" type="button" onclick={() => void loadBagPicker()}>{BAG.retry}</button>
              </div>
            {:else if bagEntries === null}
              <p class="picker-hint ui-loading" role="status">{BAG.loading}</p>
            {:else if bagVisible.length === 0}
              <p class="picker-hint">{BAG.empty.noMatch}</p>
            {:else}
              <div class="picker-grid" role="group" aria-label={BAG.filters.gridLabel}>
                {#each bagVisible as entry (entry.collection + ":" + entry.id + ":" + entry.crc)}
                  <button
                    class="picker-tile"
                    type="button"
                    disabled={busy}
                    title={`${entry.name ?? "prop"} — ${entry.collection} #${entry.id}`}
                    onclick={() => void openBagEntry(entry)}
                  >
                    <img src={api.bagThumbUrl(entry.collection, entry.id, entry.crc)} alt="" loading="lazy" />
                    <span class="picker-name">{entry.name ?? `#${entry.id}`}</span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        </div>
      </div>
    {/if}

    {#if confirmClose}
      <div class="editor-overlay">
        <div
          class="confirm"
          role="alertdialog"
          aria-modal="true"
          aria-label={EDITOR.confirmClose.label}
          tabindex="-1"
          bind:this={confirmEl}
        >
          <p>{EDITOR.confirmClose.text}</p>
          <div class="confirm-actions">
            <button class="btn" type="button" onclick={cancelClose}>{EDITOR.confirmClose.cancel}</button>
            <button class="btn primary" type="button" onclick={discardAndClose}>{EDITOR.confirmClose.discard}</button>
          </div>
        </div>
      </div>
    {/if}

    <input
      class="hidden-file"
      type="file"
      accept="image/png,image/jpeg,image/bmp,image/gif,image/webp"
      aria-label={EDITOR.file.openImage}
      bind:this={fileInput}
      onchange={onImageChosen}
    />
  </div>
</div>

<style>
  .editor-dialog {
    position: relative;
    width: min(1180px, calc(100vw - 24px));
    height: min(820px, calc(100vh - 24px));
    max-width: none;
    max-height: none;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  /* ---------- Menu bar ---------- */

  .editor-menubar {
    position: relative;
    z-index: 20;
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    background: var(--surface-3);
    border-bottom: 1px solid var(--line-strong);
    border-radius: var(--r-md) var(--r-md) 0 0;
  }

  .editor-title {
    flex: none;
    padding: 0 var(--sp-2);
    font-size: var(--fs-xs);
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--amber);
  }

  .menu-bar {
    display: flex;
    gap: 1px;
  }

  .menu-root {
    position: relative;
  }

  .menu-top {
    padding: var(--sp-1) var(--sp-2);
    background: none;
    border: 1px solid transparent;
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    color: var(--text-1);
    transition: background var(--t-fast), color var(--t-fast);
  }

  .menu-top:hover,
  .menu-top.open {
    background: var(--surface-4);
    color: var(--text-0);
  }

  .menu-pop {
    position: absolute;
    top: calc(100% + 3px);
    left: 0;
    z-index: 30;
    min-width: 228px;
    padding: var(--sp-1);
    background: var(--surface-2);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-md);
    box-shadow: var(--shadow-panel);
    animation: menu-in 90ms var(--ease);
  }

  @keyframes menu-in {
    from {
      opacity: 0;
      transform: translateY(-3px);
    }
  }

  .menu-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-4);
    width: 100%;
    padding: var(--sp-1) var(--sp-2);
    background: none;
    border: 0;
    border-radius: var(--r-sm);
    color: var(--text-1);
    font-size: var(--fs-sm);
    text-align: left;
  }

  .menu-item:hover:not(:disabled),
  .menu-item:focus-visible:not(:disabled) {
    background: var(--surface-4);
    color: var(--text-0);
  }

  .menu-item:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .menu-key {
    font-size: var(--fs-xs);
    color: var(--text-3);
  }

  .menu-sep {
    height: 1px;
    margin: var(--sp-1) var(--sp-2);
    background: var(--line);
  }

  .menu-info {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: var(--sp-2);
    margin-bottom: var(--sp-1);
    background: var(--bg-void);
    border: 1px solid var(--line);
    border-radius: var(--r-sm);
    font-size: var(--fs-xs);
  }

  .menu-info > div {
    display: flex;
    justify-content: space-between;
    gap: var(--sp-3);
  }

  .menu-info span {
    color: var(--text-3);
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .menu-info b {
    color: var(--text-1);
    font-weight: 400;
    text-align: right;
  }

  .menu-catch {
    position: fixed;
    inset: 0;
    z-index: 15;
  }

  .editor-working {
    font-size: var(--fs-xs);
    color: var(--cyan);
    animation: pulse 1s infinite var(--ease);
  }

  @keyframes pulse {
    50% {
      opacity: 0.4;
    }
  }

  .save-ind {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    flex: none;
    padding: var(--sp-1) var(--sp-2);
    background: var(--bg-void);
    border: 1px solid var(--line);
    border-radius: var(--r-sm);
    font-size: var(--fs-xs);
    color: var(--text-3);
  }

  .save-ind .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-3);
  }

  .save-ind.dirty {
    color: var(--amber);
    border-color: var(--amber-dim);
  }

  .save-ind.dirty .dot {
    background: var(--amber);
    box-shadow: 0 0 8px var(--line-glow);
  }

  .save-ind.clean {
    color: var(--text-2);
  }

  /* ---------- New-canvas bar ---------- */

  .new-bar {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-2) var(--sp-3);
    background: var(--surface-1);
    border-bottom: 1px solid var(--line);
    font-size: var(--fs-xs);
    color: var(--text-2);
  }

  .new-label {
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--amber);
  }

  .new-bar label {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
  }

  .new-bar input {
    width: 72px;
    background: var(--bg-void);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-sm);
    padding: var(--sp-1);
    font-variant-numeric: tabular-nums;
  }

  .new-bar input:focus {
    border-color: var(--amber-dim);
    box-shadow: 0 0 0 2px var(--amber-soft);
  }

  .new-hint {
    color: var(--text-3);
  }

  /* ---------- Error bar ---------- */

  .editor-error {
    margin: 0;
    border-radius: 0;
    border-left: 0;
    border-right: 0;
  }

  /* ---------- Body ---------- */

  .editor-body {
    flex: 1;
    display: grid;
    grid-template-columns: minmax(0, 1fr) 276px;
    min-height: 0;
    background: var(--line);
    gap: 1px;
  }

  .editor-main {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr);
    min-width: 0;
    min-height: 0;
    background: var(--bg-app);
  }

  .canvas-toolbar {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-2);
    background: var(--surface-2);
    border-bottom: 1px solid var(--line);
    font-size: var(--fs-xs);
    color: var(--text-2);
  }

  .canvas-toolbar .readout {
    min-width: 3.6em;
    color: var(--text-1);
    font-variant-numeric: tabular-nums;
  }

  .frame-nav .btn {
    min-width: 30px;
  }

  .frame-readout {
    font-variant-numeric: tabular-nums;
    color: var(--text-1);
  }

  .canvas-hint {
    color: var(--text-3);
  }

  /* ---------- Panels ---------- */

  .editor-panels {
    display: grid;
    grid-template-rows: auto minmax(0, 1fr);
    min-height: 0;
    background: var(--surface-1);
  }

  .panel-tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 2px;
    padding: var(--sp-1);
    background: var(--surface-2);
    border-bottom: 1px solid var(--line);
  }

  .panel-tab {
    padding: var(--sp-1) var(--sp-2);
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--r-sm);
    font-size: var(--fs-xs);
    color: var(--text-2);
    transition: background var(--t-fast), color var(--t-fast), border-color var(--t-fast);
  }

  .panel-tab:hover {
    color: var(--text-0);
    background: var(--surface-3);
  }

  .panel-tab.on {
    color: var(--amber);
    background: var(--amber-soft);
    border-color: var(--amber-dim);
  }

  .panel-body {
    min-height: 0;
    overflow-y: auto;
  }

  /* ---------- Status line ---------- */

  .editor-status {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-1) var(--sp-3);
    background: var(--surface-2);
    border-top: 1px solid var(--line-strong);
    font-size: var(--fs-xs);
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
    min-height: 26px;
    overflow: hidden;
  }

  .status-item {
    white-space: nowrap;
  }

  .status-sep {
    width: 1px;
    height: 14px;
    background: var(--line-strong);
    flex: none;
  }

  .status-note {
    color: var(--cyan);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .status-note.warn {
    color: var(--amber);
  }

  .status-note.error {
    color: var(--red);
  }

  .editor-status .depth {
    color: var(--text-3);
    flex: none;
  }

  /* ---------- Overlays ---------- */

  .editor-overlay {
    position: absolute;
    inset: 0;
    z-index: 90;
    display: grid;
    place-items: center;
    background: rgba(3, 5, 8, 0.7);
    border-radius: var(--r-md);
  }

  .picker {
    width: min(680px, calc(100% - 32px));
    max-height: calc(100% - 48px);
    display: grid;
    grid-template-rows: auto minmax(0, 1fr);
    background: var(--surface-2);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-md);
    box-shadow: var(--shadow-panel);
    overflow: hidden;
  }

  .picker-body {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    padding: var(--sp-3);
    min-height: 0;
  }

  .picker-search {
    flex: none;
    background: var(--bg-void);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-sm);
    padding: var(--sp-1) var(--sp-2);
    font-size: var(--fs-sm);
  }

  .picker-search:focus {
    border-color: var(--amber-dim);
    box-shadow: 0 0 0 2px var(--amber-soft);
  }

  .picker-hint {
    margin: 0;
    font-size: var(--fs-sm);
    color: var(--text-2);
  }

  .picker-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, 72px);
    gap: var(--sp-1);
    align-content: start;
    justify-content: start;
    min-height: 0;
    overflow-y: auto;
    padding: var(--sp-2);
    background: var(--bg-void);
    border: 1px solid var(--line);
    border-radius: var(--r-sm);
  }

  .picker-tile {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    width: 72px;
    padding: 2px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--r-sm);
  }

  .picker-tile:hover:not(:disabled),
  .picker-tile:focus-visible:not(:disabled) {
    border-color: var(--amber);
    background: var(--amber-soft);
  }

  .picker-tile:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .picker-tile img {
    width: 48px;
    height: 48px;
    object-fit: contain;
    image-rendering: pixelated;
  }

  .picker-name {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--fs-xs);
    color: var(--text-2);
  }

  .confirm {
    width: min(420px, calc(100% - 32px));
    padding: var(--sp-4);
    background: var(--surface-2);
    border: 1px solid var(--line-strong);
    border-radius: var(--r-md);
    box-shadow: var(--shadow-panel);
  }

  .confirm p {
    margin: 0 0 var(--sp-3);
    font-size: var(--fs-sm);
    color: var(--text-1);
    line-height: 1.7;
  }

  .confirm-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--sp-2);
  }

  .hidden-file {
    display: none;
  }
</style>
